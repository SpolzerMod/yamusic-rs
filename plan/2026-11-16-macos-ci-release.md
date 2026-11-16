# CI: сборка macOS и публикация в релиз

Дата: 2026-11-16. Статус: согласовано с пользователем, в работе.

## Задача

GitHub Actions должен собирать macOS-приложение (`.app`) и прикладывать его к тому же
релизу, что и Windows `.exe`, при пуше тега `v*`. Сейчас workflow (`build.yml`) собирает
только Windows-`exe` и создаёт релиз с одним файлом.

## Решения (согласованы)

- **Подпись:** без Developer ID, только ad-hoc (`codesign --sign -`, уже в
  `tools/make_app.sh`). Скачанный `.app` будет под карантином Gatekeeper — в README
  добавляем инструкцию (right-click → Open или `xattr -d com.apple.quarantine`).
- **Архитектура:** только `arm64` (Apple Silicon). Intel-маки не поддерживаются этой сборкой.
- **Формат:** `yamusic-rs-macos-arm64.zip` с `.app` внутри (упаковка `ditto`, чтобы
  сохранить структуру бандла и атрибуты).
- **Структура workflow:** отдельные job'ы `windows` и `macos` собирают и кладут артефакты,
  общий job `release` (только на тег) скачивает оба и создаёт один релиз. Это исключает
  гонку двух job'ов за один тег.
- **Ветка:** реализация на `memory-audit` (macos-порт + оптимизации памяти), чтобы релиз
  включал и то, и другое. Ветку `macos` не трогаем (там работает другой агент).

## План реализации

1. `.github/workflows/build.yml`:
   - `windows`: без изменений (clippy/test/build/SHA/upload), убрать шаг «Релиз».
   - `macos`: `runs-on: macos-latest`; checkout, toolchain, cache, `cargo build --release`,
     `tools/make_app.sh`, `ditto -c -k --keepParent ... yamusic-rs-macos-arm64.zip`,
     `upload-artifact`.
   - `release`: `if: startsWith(github.ref,'refs/tags/v')`, `needs: [windows, macos]`,
     `permissions: contents: write`, `download-artifact` (merge), `gh release create` с
     обоими файлами.
2. `README.md`: короткая инструкция установки macOS (распаковать, перетащить в Applications,
   обход Gatekeeper).
3. Проверка: тег `v1.0.2` в форке (совпадает с версией `Cargo.toml`; суффиксный тег
   сломал бы шаг «Тег совпадает с версией») → в Release появляются `yamusic-rs.exe` и
   `yamusic-rs-macos-arm64.zip`; zip скачивается, `.app` запускается на маке.

## Критерии готовности

- CI на теге зелёный, в релизе оба файла.
- `yamusic-rs-macos-arm64.zip` распаковывается в рабочий `.app` (проверено на этом маке).
- Windows-путь сборки/релиза не сломан.
