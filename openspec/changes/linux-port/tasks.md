# Tasks: linux-port

## 1. Каркас платформенности (сборка под Linux)

- [x] 1.1 `Cargo.toml`: femtovg-рендер, Windows-зависимости в target-specific секции,
      Linux-зависимости (mpris-server, rfd, tokio).
- [x] 1.2 `src/win.rs`: `#[cfg(windows)]` на текущий код + Linux-реализация (open_url,
      autostart, system_light_theme, pick_image через rfd, no-op оконных функций).
- [x] 1.3 `src/tray.rs`: `#[cfg(windows)]` + no-op-заглушки с тем же API.
- [x] 1.4 `src/main.rs`: single-instance через Unix-сокет, wake второго запуска, femtovg.
- [x] 1.5 `src/gui.rs`: убрать зависимость от HWND (cfg), `window.hide()` при закрытии в игру,
      глобальный `wake()`, масштаб от `w.scale_factor()`.
- [x] 1.6 `src/secret.rs`: Linux-реализация без DPAPI.
- [x] 1.7 `src/update.rs`: Linux-ветка (без exe-ассета, совет deb).
- [x] 1.8 Сборка проходит: `cargo build`, `cargo clippy --all-targets`, `cargo test`.

## 2. MPRIS

- [x] 2.1 `src/mpris.rs`: поток-сервер (tokio current-thread + LocalSet), `Player::builder`.
- [x] 2.2 Методы: play_pause/play/pause/next/prev/seek/volume/raise/quit.
- [x] 2.3 Свойства: playback_status, metadata (title/artist/album/length/artUrl с кэшем
      обложки в файле), volume, position + Seeked на смене трека.
- [x] 2.4 Интеграция: тик 250 мс в `gui.rs` шлёт снапшот; проверено через `busctl`
      (`org.mpris.MediaPlayer2.yamusic_rs`, Identity/PlaybackStatus/Volume отвечают).

## 3. Упаковка и CI

- [x] 3.1 PNG-иконки из `icons/app.ico` (16/32/48/64/128/256), `yamusic-rs.desktop`.
- [x] 3.2 `[package.metadata.deb]` + проверка `cargo deb` (+ depends: libasound2,
      libwayland-egl1 — найдены при тестовой установке).
- [x] 3.3 Linux-джоба в `.github/workflows/build.yml`.

## 4. QA на Ubuntu 26.04 (Wayland, GNOME)

- [x] 4.1 Запуск под Wayland (контейнер, GPU Iris Xe): без ошибок; single-instance:
      второй запуск будит окно и выходит. Полный сценарий с входом — руками.
- [ ] 4.2 MPRIS: виджет GNOME, медиа-клавиши, управление из shell — свойства проверены
      через busctl; визуальная проверка виджета и клавиш — руками.
- [ ] 4.3 Автозапуск, масштаб дробный (150%), femtovg-визуальный smoke-test — руками.
- [x] 4.4 deb: установка/удаление на чистом ubuntu:26.04 (контейнер), состав пакета,
      .desktop и иконки hicolor. Установка в живую систему — руками.
- [x] 4.5 Отрисовка и ввод: окно входа нарисовано целиком (шрифты и иконки веб-версии),
      код входа — в нижнем регистре, как его принимает сервис.
- [x] 4.6 Клики под X11-бэкендом (контейнер, `xdotool`): «Скопировать код» и
      «Открыть ya.ru/device» доходят до обработчиков (в логе `клик: …`); копирование
      идёт через `xclip`, когда `wl-copy` недоступен.
- [ ] 4.7 Клики и обновление кадра под Wayland — проверить руками после исправления
      масштаба (убраны синтетические `ScaleFactorChanged`/`Resized`).
- [x] 4.8 Найдено и исправлено при QA: `wl-clipboard` в зависимостях, путь скачивания
      (`xdg-user-dir MUSIC`), имя устройства (`$HOSTNAME`), Discord-поток на Linux,
      код входа в нижнем регистре, ожидание `xdg-open` в фоне.
