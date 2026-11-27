//! Действия с карточки плейлиста, подкаста или книги без перехода на страницу.

use super::*;

fn playlist_parts(target: &str) -> Option<(&str, &str)> {
    let (owner, kind) = target.strip_prefix("playlist:")?.split_once(':')?;
    (owner.parse::<u64>().is_ok() && kind.parse::<i64>().is_ok()).then_some((owner, kind))
}

/// Немузыкальный альбом (`isNonMusic` оригинала, модуль 8413).
pub(super) fn is_nonmusic(kind: &str) -> bool {
    matches!(kind, "podcast" | "audiobook" | "fairy-tale")
}

/// Подкаст, книга или сказка с карточки: `album:ID:тип`. Тип нужен подписи и тосту подписки.
fn album_parts(target: &str) -> Option<(&str, &str)> {
    let (id, kind) = target.strip_prefix("album:")?.split_once(':')?;
    (id.parse::<u64>().is_ok() && is_nonmusic(kind)).then_some((id, kind))
}

pub(super) fn card_target(target: &str, kind: &str) -> SharedString {
    if playlist_parts(target).is_some() {
        return target.into();
    }
    let album = format!("{target}:{kind}");
    if album_parts(&album).is_some() { album.into() } else { SharedString::new() }
}

/// «Нравится» в меню. У подкаста и книги вместо лайка подписка, как ContextMenu подписки
/// оригинала (модули 44988 и 108).
pub(super) fn like_label(album_kind: Option<&str>, liked: bool) -> &'static str {
    match (album_kind, liked) {
        (Some("podcast"), false) => "Подписаться на подкаст",
        (Some("podcast"), true) => "Вы подписаны",
        (Some("audiobook"), false) => "Добавить на полку",
        (Some("audiobook"), true) => "Убрать с полки",
        (Some("fairy-tale"), false) => "Нравится",
        (Some("fairy-tale"), true) => "Не нравится",
        (_, false) => "Нравится",
        (_, true) => "Удалить из коллекции",
    }
}

impl Ctx {
    pub(super) fn update_playlist_cards(&self) {
        let ui = self.ui();
        let app = ui.global::<App>();
        app.set_playlist_card_revision(app.get_playlist_card_revision().wrapping_add(1));
    }

    fn playlist_card_state(&self, target: &str) -> PlaylistCardState {
        let st = self.st.borrow();
        // Трейлера у подкаста и книги нет: в меню альбома оригинала он только у музыки
        // (модуль 95756), на карточке — при `trailer.available`, а у них он false.
        if let Some((id, _)) = album_parts(target) {
            return PlaylistCardState {
                pinned: st.pins.iter().any(|p| p.target.strip_prefix("album:") == Some(id)),
                liked: st.liked_albums.contains(id),
                can_like: !st.uid.is_empty(),
                trailer: false,
            };
        }
        PlaylistCardState {
            pinned: st.pins.iter().any(|p| p.target == target),
            liked: st.liked_playlists.contains(target),
            can_like: !st.uid.is_empty() && playlist_parts(target).is_some_and(|(owner, _)| owner != st.uid),
            trailer: true,
        }
    }

    fn playlist_card_action(self: &Rc<Self>, action: PlaylistCardAction, target: &str, title: &str, x: f32, y: f32) {
        if let Some((id, kind)) = album_parts(target) {
            let album = format!("album:{id}");
            match action {
                PlaylistCardAction::Play => self.play_target(&album, title),
                PlaylistCardAction::Pin => self.toggle_pin(album),
                PlaylistCardAction::Like => {
                    if self.playlist_card_state(target).can_like {
                        let cover = self.st.borrow().cards.values().flatten().find(|c| c.target == album).and_then(|c| c.covers.first().cloned());
                        let cover = cover.or_else(|| landing_ui::card_cover(&album));
                        self.toggle_subscription(id, kind, title, cover.as_deref());
                    }
                }
                PlaylistCardAction::Trailer => {}
                PlaylistCardAction::Menu => self.open_playlist_card_menu(target, title, x, y),
            }
            return;
        }
        let Some((owner, kind)) = playlist_parts(target) else { return };
        match action {
            PlaylistCardAction::Play => self.play_target(target, title),
            PlaylistCardAction::Pin => self.toggle_pin(target.to_string()),
            PlaylistCardAction::Like => {
                if self.playlist_card_state(target).can_like {
                    self.toggle_playlist_like(target, title);
                }
            }
            PlaylistCardAction::Trailer => self.play_trailer(format!("users/{owner}/playlists/{kind}")),
            PlaylistCardAction::Menu => self.open_playlist_card_menu(target, title, x, y),
        }
    }

    fn open_playlist_card_menu(&self, target: &str, title: &str, x: f32, y: f32) {
        let state = self.playlist_card_state(target);
        let ui = self.ui();
        let icons = ui.global::<Icons>();
        let mut entries = vec![self.menu_entry(
            "playlist-card-pin",
            if state.pinned { "Открепить" } else { "Закрепить" },
            if state.pinned { icons.get_m_unpin() } else { icons.get_m_pin() },
            false,
        )];
        if state.can_like {
            entries.push(self.menu_entry(
                "playlist-card-like",
                like_label(album_parts(target).map(|(_, kind)| kind), state.liked),
                if state.liked { icons.get_m_liked() } else { icons.get_m_like() },
                false,
            ));
        }
        if state.trailer {
            entries.push(self.menu_entry("playlist-card-trailer", "Трейлер", icons.get_m_trailer(), false));
        }
        self.st.borrow_mut().playlist_card_menu = Some((target.to_string(), title.to_string()));
        self.show_menu(entries, x, y, true);
    }

    pub(super) fn playlist_card_menu_action(self: &Rc<Self>, action: &str) {
        let action = match action {
            "pin" => PlaylistCardAction::Pin,
            "like" => PlaylistCardAction::Like,
            "trailer" => PlaylistCardAction::Trailer,
            _ => return,
        };
        let selected = self.st.borrow_mut().playlist_card_menu.take();
        if let Some((target, title)) = selected {
            self.playlist_card_action(action, &target, &title, 0.0, 0.0);
        }
    }

    fn set_playlist_liked(&self, target: &str, liked: bool) {
        {
            let mut st = self.st.borrow_mut();
            if liked {
                st.liked_playlists.insert(target.to_string());
            } else {
                st.liked_playlists.remove(target);
            }
        }
        let ui = self.ui();
        let app = ui.global::<App>();
        if app.get_page() == Page::List && self.list_target().as_deref() == Some(target) {
            app.set_list_liked(liked);
        }
        self.update_playlist_cards();
    }

    pub(super) fn toggle_playlist_like(self: &Rc<Self>, target: &str, title: &str) {
        let Some((owner, kind)) = playlist_parts(target) else { return };
        let id = format!("{owner}:{kind}");
        let request = Arc::new(());
        let (net, was_liked) = {
            let mut st = self.st.borrow_mut();
            let Some(net) = st.net.clone() else { return };
            if st.pending_playlist_likes.contains_key(target) {
                return;
            }
            st.pending_playlist_likes.insert(target.to_string(), request.clone());
            (net, st.liked_playlists.contains(target))
        };
        self.set_playlist_liked(target, !was_liked);
        let (target, title) = (target.to_string(), title.to_string());
        std::thread::spawn(move || {
            let result = net.api.like_entity(&net.uid, "playlists", &id, was_liked);
            let _ = slint::invoke_from_event_loop(move || with_ctx(|c| {
                {
                    let mut st = c.st.borrow_mut();
                    if !st.pending_playlist_likes.get(&target).is_some_and(|pending| Arc::ptr_eq(pending, &request)) {
                        return;
                    }
                    st.pending_playlist_likes.remove(&target);
                }
                match result {
                    Ok(()) => {
                        net.refresh("collection");
                        c.toast(if was_liked { format!("«{title}» удалён из коллекции") } else { format!("«{title}» в коллекции") });
                    }
                    Err(e) => {
                        c.set_playlist_liked(&target, was_liked);
                        c.toast(format!("Не удалось сохранить: {e}"));
                    }
                }
            }));
        });
    }

    fn set_album_liked(&self, id: &str, liked: bool) {
        {
            let mut st = self.st.borrow_mut();
            if liked {
                st.liked_albums.insert(id.to_string());
            } else {
                st.liked_albums.remove(id);
            }
        }
        let ui = self.ui();
        let app = ui.global::<App>();
        if app.get_page() == Page::List && self.list_target().is_some_and(|t| t.strip_prefix("album:") == Some(id)) {
            app.set_list_liked(liked);
        }
        self.update_playlist_cards();
    }

    /// Подписка на подкаст, книгу или сказку — лайк альбома. Тост с обложкой и ссылками
    /// (`notifications-info.*-collection`, модуль 96295), ошибка —
    /// `error-messages.error-during-action` (модуль 92966).
    pub(super) fn toggle_subscription(self: &Rc<Self>, id: &str, kind: &str, title: &str, cover: Option<&str>) {
        let Some(net) = self.st.borrow().net.clone() else { return };
        let liked = !self.st.borrow().liked_albums.contains(id);
        self.set_album_liked(id, liked);
        let (id, kind, title, cover) = (id.to_string(), kind.to_string(), title.to_string(), cover.map(String::from));
        std::thread::spawn(move || {
            let saved = net.api.like_entity(&net.uid, "albums", &id, !liked);
            if saved.is_ok() {
                net.refresh("collection");
            }
            let _ = slint::invoke_from_event_loop(move || with_ctx(|c| match saved {
                Ok(()) => c.subscribe_toast(&kind, &title, &id, cover.as_deref(), liked),
                Err(e) => {
                    log(format!("подписка на {id}: {e}"));
                    // Не сохранилось — подписка остаётся прежней.
                    c.set_album_liked(&id, !liked);
                    c.toast("При выполнении действия произошла ошибка");
                }
            }));
        });
    }
}

pub(super) fn register(app: &App) {
    app.on_playlist_card_state(|target, _| {
        CTX.with(|ctx| ctx.borrow().as_ref().map(|c| c.playlist_card_state(&target)).unwrap_or_default())
    });
    app.on_playlist_card_action(|action, target, title, x, y| {
        with_ctx(|c| c.playlist_card_action(action, &target, &title, x, y));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_target_only_for_playlists_and_nonmusic_albums() {
        assert_eq!(card_target("playlist:42:1003", ""), "playlist:42:1003");
        assert_eq!(card_target("album:18837614", "podcast"), "album:18837614:podcast");
        assert_eq!(album_parts("album:18837614:audiobook"), Some(("18837614", "audiobook")));
        assert_eq!(card_target("album:123", ""), "");
        assert_eq!(card_target("album:123", "single"), "");
        assert_eq!(card_target("artist:5", ""), "");
    }
}
