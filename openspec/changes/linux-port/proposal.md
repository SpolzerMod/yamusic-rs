# Proposal: linux-port — нативный порт на Ubuntu 26.04

## Зачем

README позиционирует Linux-поддержку «в планах» (`README.md:118`). Ветка `linux-port` делает
нативный Linux-билд: deb-пакет, MPRIS вместо трея, femtovg-рендер вместо Skia.

## Принятые решения (согласованы)

- **Рендер**: `renderer-femtovg`, прямой `skia-safe` из `[dependencies]` убирается. Проверено на
  целевой машине: Intel Iris Xe, Mesa 26.0.8, OpenGL 4.6, EGL 1.5, Wayland — запас есть.
  Windows-сборка в этой ветке вторична (слияние с main потребует feature-флага рендера).
- **Трей**: не портируется. Закрытие окна при игре → `window.hide()` (процесс живёт),
  повторный запуск поднимает окно, автозапуск — флаг `--hidden` (алиас `--tray` сохранён).
- **Управление без окна**: MPRIS 2 через крейт `mpris-server` 0.9 (0.10/0.11 не собираются —
  пинну 0.9). Обложка, название, кнопки ⏮▶⏭ в GNOME Shell + системные мультимедиа-клавиши.
- **Токен**: без шифрования, `~/.config/yamusic-rs/config.json` с правами `0600`.
- **Тема**: тёмная по умолчанию (config `theme=light` продолжает работать).
- **update.rs**: только проверка версии + ссылка на релизы; self-update остаётся Windows-only.
- **Discord**: не трогается.
- **Кастомная обложка плейлиста**: диалог выбора через `rfd` (xdg-desktop-portal).

## Изменения

### Код

- `Cargo.toml`: slint `renderer-skia` → `renderer-femtovg`; `skia-safe` и `windows-sys` —
  в `[target.'cfg(windows)'.dependencies]`; новые Linux-зависимости: `mpris-server 0.9`,
  `rfd 0.15`, `tokio 1 (rt)`.
- `src/win.rs`: существующий код под `#[cfg(windows)]`; Linux-реализация того же API:
  autostart через `~/.config/autostart/*.desktop`, `open_url` через `xdg-open`, тема через
  `gsettings`, no-op для оконных функций.
- `src/tray.rs`: весь модуль под `#[cfg(windows)]`; Linux-заглушки с тем же публичным API.
- `src/main.rs`: single-instance и wake через Unix-сокет (`$XDG_RUNTIME_DIR`), рендерер femtovg.
- `src/gui.rs`: `hwnd()` → 0 на Linux; `finish_reveal`/`apply_ui_scale`/`on_close` без Win32;
  глобальный `wake()`; MPRIS-синхронизация в тике плеера.
- `src/secret.rs`: Linux — base64 c маркером `RAW`, без DPAPI.
- `src/update.rs`: Linux — без поиска exe-ассета, `install()` советует deb.
- `src/mpris.rs` (новый): поток с tokio current-thread + `mpris_server::Player`.

### Упаковка

- `Cargo.toml` `[package.metadata.deb]` (cargo-deb): бинарь, `.desktop`, иконки hicolor,
  зависимости `libegl1/libgl1/libwayland-client0/libxkbcommon0/libfontconfig1/xdg-utils`.
- `.github/workflows/build.yml`: Linux-джоба (clippy/test/build + deb в артефактах).

## Риски

- femtovg может рисовать тени/блюр иначе, чем skia-software — визуальный smoke-test обязателен.
- Токен в конфиге читаем любым процессом того же пользователя (осознанный компромисс).
- GNOME-трей невозможен без AppIndicator-расширения — закрывается MPRIS, трей отложен.
