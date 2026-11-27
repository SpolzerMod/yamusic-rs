//! Edge docking of the Windows mini-player. The native window stays within the monitor work area.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::winit_030::{winit::dpi::LogicalSize, WinitWindowAccessor};
use slint::{ComponentHandle, Timer, TimerMode};

use super::{App, AppWindow, MINI_H, MINI_W};
use crate::sys;

#[derive(Clone, Copy)]
enum Edge { Left = 1, Right, Top, Bottom }

#[derive(Clone, Copy)]
struct Attachment {
    edge: Edge,
    anchor: (i32, i32),
}

type Bounds = (i32, i32, i32, i32);

impl Attachment {
    fn at_cursor(scale: f32) -> Option<Self> {
        let anchor = sys::cursor_position()?;
        let (x, y) = anchor;
        let (l, t, r, b) = sys::work_area_at(anchor)?;
        let (distance, edge) = [
            ((x - l).abs(), Edge::Left), ((r - 1 - x).abs(), Edge::Right),
            ((y - t).abs(), Edge::Top), ((b - 1 - y).abs(), Edge::Bottom),
        ].into_iter().min_by_key(|(distance, _)| *distance)?;
        (distance <= (24.0 * scale).round() as i32).then_some(Self { edge, anchor })
    }

    fn bounds(self, scale: f32, exposure: f32) -> Option<Bounds> {
        let (l, t, r, b) = sys::work_area_at(self.anchor)?;
        let (small_w, small_h) = match self.edge {
            Edge::Left | Edge::Right => (24.0, 90.0),
            Edge::Top | Edge::Bottom => (104.0, 24.0),
        };
        let w = (((MINI_W - small_w) * exposure + small_w) * scale).round() as i32;
        let h = (((MINI_H - small_h) * exposure + small_h) * scale).round() as i32;
        let (x, y) = self.anchor;
        let centered_x = (x - w / 2).clamp(l, (r - w).max(l));
        let centered_y = (y - h / 2).clamp(t, (b - h).max(t));
        let (x, y) = match self.edge {
            Edge::Left => (l, centered_y),
            Edge::Right => (r - w, centered_y),
            Edge::Top => (centered_x, t),
            Edge::Bottom => (centered_x, b - h),
        };
        Some((x, y, w, h))
    }
}

#[derive(Clone, Copy)]
struct Slide {
    from: f32,
    opened: bool,
    since: Instant,
}

#[derive(Default)]
pub(super) struct Dock {
    attachment: Cell<Option<Attachment>>,
    dragging: Cell<bool>,
    ignore_hover: Cell<bool>,
    preview_until: Cell<Option<Instant>>,
    seen_track: Cell<i32>,
    slide: Cell<Option<Slide>>,
    exposure: Cell<f32>,
    last_bounds: Cell<Option<Bounds>>,
    timer: Timer,
}

impl Dock {
    pub(super) fn start_drag(self: &Rc<Self>, ui: &AppWindow) {
        self.clear(ui);
        self.dragging.set(true);
        self.start(ui);
    }

    pub(super) fn clear(&self, ui: &AppWindow) {
        self.timer.stop();
        self.attachment.set(None);
        self.dragging.set(false);
        self.slide.set(None);
        self.last_bounds.set(None);
        self.preview_until.set(None);
        ui.global::<App>().set_mini_edge(0);
        ui.global::<App>().set_mini_tab(false);
    }

    pub(super) fn preview(&self) {
        if self.attachment.get().is_some() {
            self.ignore_hover.set(false);
            self.preview_until.set(Some(Instant::now() + Duration::from_millis(1800)));
        }
    }

    fn start(self: &Rc<Self>, ui: &AppWindow) {
        let ui = ui.as_weak();
        let dock = Rc::downgrade(self);
        self.timer.start(TimerMode::Repeated, Duration::from_millis(24), move || {
            if let (Some(ui), Some(dock)) = (ui.upgrade(), dock.upgrade()) { dock.tick(&ui); }
        });
    }

    fn tick(&self, ui: &AppWindow) {
        let app = ui.global::<App>();
        let w = ui.window();
        if !app.get_mini() {
            self.clear(ui);
            return;
        }
        if !sys::is_shown(w) || sys::is_minimized(w) { return; }
        let scale = sys::scale(w);
        if self.dragging.get() {
            if sys::left_button_down() { return; }
            self.dragging.set(false);
            let Some(attachment) = Attachment::at_cursor(scale) else { self.clear(ui); return };
            self.attachment.set(Some(attachment));
            self.ignore_hover.set(true);
            self.seen_track.set(app.get_mini_track_revision());
            self.exposure.set(1.0);
            self.slide.set(Some(Slide { from: 1.0, opened: false, since: Instant::now() }));
            app.set_mini_edge(attachment.edge as i32);
            w.with_winit_window(|window| window.set_min_inner_size(Some(LogicalSize::new(24.0, 24.0))));
        }
        let Some(attachment) = self.attachment.get() else { return };
        if self.seen_track.replace(app.get_mini_track_revision()) != app.get_mini_track_revision() && app.get_has_track() {
            self.preview();
        }

        let now = Instant::now();
        let hover = self.last_bounds.get().is_some_and(|(x, y, w, h)| {
            sys::cursor_position().is_some_and(|(mx, my)| (x..x + w).contains(&mx) && (y..y + h).contains(&my))
        });
        if !hover && self.exposure.get() == 0.0 { self.ignore_hover.set(false); }
        let opened = (hover && !self.ignore_hover.get()) || self.preview_until.get().is_some_and(|until| until > now);
        if self.slide.get().is_none_or(|slide| slide.opened != opened) {
            self.slide.set(Some(Slide { from: self.exposure.get(), opened, since: now }));
        }
        let Some(slide) = self.slide.get() else { return };
        let t = if app.get_motion() { (now.duration_since(slide.since).as_secs_f32() / 0.20).min(1.0) } else { 1.0 };
        let ease = t * t * (3.0 - 2.0 * t);
        let target = if slide.opened { 1.0 } else { 0.0 };
        let exposure = slide.from + (target - slide.from) * ease;
        self.exposure.set(exposure);
        app.set_mini_tab(!opened && t >= 1.0);
        if let Some(bounds) = attachment.bounds(scale, exposure) {
            if self.last_bounds.replace(Some(bounds)) != Some(bounds) { sys::move_mini(w, bounds); }
        }
    }
}
