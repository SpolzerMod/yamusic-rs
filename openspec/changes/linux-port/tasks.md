# Tasks: linux-port

## 1. Каркас платформенности (сборка под Linux)

- [ ] 1.1 `Cargo.toml`: femtovg-рендер, Windows-зависимости в target-specific секции,
      Linux-зависимости (mpris-server, rfd, tokio).
- [ ] 1.2 `src/win.rs`: `#[cfg(windows)]` на текущий код + Linux-реализация (open_url,
      autostart, system_light_theme, pick_image через rfd, no-op оконных функций).
- [ ] 1.3 `src/tray.rs`: `#[cfg(windows)]` + no-op-заглушки с тем же API.
- [ ] 1.4 `src/main.rs`: single-instance через Unix-сокет, wake второго запуска, femtovg.
- [ ] 1.5 `src/gui.rs`: убрать зависимость от HWND (cfg), `window.hide()` при закрытии в игру,
      глобальный `wake()`, масштаб от `w.scale_factor()`.
- [ ] 1.6 `src/secret.rs`: Linux-реализация без DPAPI.
- [ ] 1.7 `src/update.rs`: Linux-ветка (без exe-ассета, совет deb).
- [ ] 1.8 Сборка проходит: `cargo build`, `cargo clippy --all-targets`, `cargo test`.

## 2. MPRIS

- [ ] 2.1 `src/mpris.rs`: поток-сервер (tokio current-thread + LocalSet), `Player::builder`.
- [ ] 2.2 Методы: play_pause/play/pause/next/prev/seek/volume/raise/quit.
- [ ] 2.3 Свойства: playback_status, metadata (title/artist/album/length/artUrl с кэшем
      обложки в файле), volume, position + Seeked на смене трека.
- [ ] 2.4 Интеграция: тик 250 мс в `gui.rs` шлёт снапшот; проверка через `busctl`/GNOME.

## 3. Упаковка и CI

- [ ] 3.1 PNG-иконки из `icons/app.ico` (16/32/48/64/128/256), `yamusic-rs.desktop`.
- [ ] 3.2 `[package.metadata.deb]` + проверка `cargo deb`.
- [ ] 3.3 Linux-джоба в `.github/workflows/build.yml`.

## 4. QA на Ubuntu 26.04 (Wayland, GNOME)

- [ ] 4.1 Запуск, вход по коду устройства, воспроизведение, свёрнутое закрытие при игре.
- [ ] 4.2 MPRIS: виджет GNOME, медиа-клавиши, управление из shell.
- [ ] 4.3 Автозапуск, масштаб дробный (150%), femtovg-визуальный smoke-test.
- [ ] 4.4 deb: установка/удаление, иконки и .desktop в меню.
