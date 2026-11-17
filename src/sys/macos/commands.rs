//! Команды из нативных обработчиков доставляются только в живой интерфейс Slint.
use objc2::MainThreadMarker;
use slint::ComponentHandle;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// Общий протокол системной интеграции. Источники остальных команд добавляются
/// на этапах подключения меню и MediaPlayer; сейчас подключён повторный запуск.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MacCommand {
    ShowWindow,
    CloseWindow,
    Quit,
    Settings,
    Play,
    Pause,
    PlayPause,
    Next,
    Prev,
    Seek(f32),
    Mini,
    Edit(char),
    EnsureVisible,
}

#[derive(Clone)]
struct Lifetime(Arc<AtomicBool>);

impl Lifetime {
    fn new() -> Self {
        Self(Arc::new(AtomicBool::new(true)))
    }
    fn is_active(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    fn invalidate(&self) {
        self.0.store(false, Ordering::Release);
    }
    fn dispatch(&self, action: impl FnOnce()) {
        if self.is_active() {
            action();
        }
    }
}

/// Клонируемый отправитель для сокета, а затем меню и MediaPlayer.
#[derive(Clone)]
pub(super) struct CommandSender(Arc<dyn Fn(MacCommand) + Send + Sync>);

impl CommandSender {
    pub(super) fn send(&self, command: MacCommand) {
        (self.0)(command);
    }
}

/// Сохраняется в gui::run до выхода из event loop. MainThreadMarker запрещает
/// перенос владельца нативных ресурсов на фоновый поток.
pub struct MacIntegration {
    lifetime: Lifetime,
    _main_thread: MainThreadMarker,
    native_started: bool,
}

impl MacIntegration {
    pub fn install<T: ComponentHandle + 'static>(ui: &T, on_command: impl Fn(T, MacCommand) + Send + Sync + 'static) -> Self {
        let main_thread = MainThreadMarker::new().expect("интеграция macOS создаётся в главном потоке");
        let lifetime = Lifetime::new();
        let weak = ui.as_weak();
        let callback = Arc::new(on_command);
        let active = lifetime.clone();
        // Сокет читается в фоне; все обращения к GUI — через очередь Slint.
        let sender = CommandSender(Arc::new(move |command| {
            if !active.is_active() {
                return;
            }
            let active = active.clone();
            let callback = callback.clone();
            // Действия текстового меню должны завершиться до следующего нажатия.
            // AppKit вызывает их в главном потоке; остальные команды остаются в очереди.
            if matches!(command, MacCommand::Edit(_)) && MainThreadMarker::new().is_some() {
                if let Some(ui) = weak.upgrade() {
                    active.dispatch(|| callback(ui, command));
                }
                return;
            }
            if let Err(error) = weak.upgrade_in_event_loop(move |ui| {
                // Команда могла попасть в очередь непосредственно перед shutdown.
                active.dispatch(|| callback(ui, command));
            }) {
                crate::config::log(format!("команда macOS не доставлена: {error}"));
            }
        }));
        let native_started = super::native::start(sender.clone());
        if !native_started {
            crate::config::log("macOS: неподдерживаемый NSApplicationDelegate; нативная интеграция не установлена");
        }
        super::instance::on_wake(move || sender.send(MacCommand::ShowWindow));
        Self {
            lifetime,
            native_started,
            _main_thread: main_thread,
        }
    }
}

impl Drop for MacIntegration {
    fn drop(&mut self) {
        // Сначала отзываем команды; будущие нативные подписки снимаются здесь же,
        // в главном потоке, до уничтожения окна и выхода из процесса.
        self.lifetime.invalidate();
        super::media::reset();
        if self.native_started {
            super::native::stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn queued_command_is_cancelled_on_shutdown() {
        let owner = Lifetime::new();
        let queued = owner.clone();
        let calls = AtomicUsize::new(0);
        owner.dispatch(|| {
            calls.fetch_add(1, Ordering::Relaxed);
        });
        owner.invalidate();
        queued.dispatch(|| {
            calls.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn worker_observes_shutdown() {
        let owner = Lifetime::new();
        let worker = owner.clone();
        owner.invalidate();
        assert!(!std::thread::spawn(move || worker.is_active()).join().unwrap());
    }
}
