# Снижение потребления памяти yamusic-rs с ~65 МБ до ~30–40 МБ


This ExecPlan is a living document. Sections `Progress`, `Surprises & Discoveries`, `Decision Log` and `Outcomes & Retrospective` must be kept up to date as work proceeds. Файла `PLANS.md` в репозитории нет, поэтому план следует только стандарту ExecPlan. Внимание: в корне репозитория есть отдельный файл `PLAN.md` — это план другой работы (порт на macOS), к данному плану он отношения не имеет и изменять его не нужно.


## Purpose / Big Picture


Приложение «Яндекс Музыка RS» — неофициальный клиент Яндекс Музыки на Rust с интерфейсом на Slint и рендером Skia на процессоре. Его главное преимущество перед оригиналом — низкое потребление памяти: сейчас около 65 МБ против ~850 МБ у оригинала (замер описан в `README.md`, раздел «Замеры»). После выполнения этого плана приложение в том же сценарии (играет трек в качестве 320 kbps, открыт полноэкранный плеер с текстом песни) должно занимать примерно 30–40 МБ, при этом все функции сохраняются: воспроизведение, перемотка, повтор, предзагрузка следующего трека, навигация по страницам, поиск, обложки, текст песни, лёгкий режим.

Проверить результат можно замером «Private Bytes» процесса на Windows до и после изменений по методике из `README.md` (команда приведена в разделе «Concrete Steps»). Каждый майлстоун даёт измеримый вклад и проверяется отдельно, поэтому работу можно останавливать и продолжать с любого места.


## Progress


- [ ] (2026-11-15) План составлен по результатам глубокого аудита памяти (4 параллельных аудита + верификация по исходникам Slint 1.18.1). Реализация ещё не начиналась.
- [x] (2026-11-15) M0 (локальная часть): worktree `memory-audit` создан, ребейзнут на `mac-port-snapshot` (`2ac8f0c`), план перенесён и адаптирован; `cargo check` проходит локально. Осталось: базовый замер памяти на Windows и подтверждение CI (выполняется пользователем на Windows).
- [x] (2026-11-15) M1: аудио — `Bytes` вместо `Vec<u8>` (api.rs, player.rs), убрана копия `Arc<[u8]>::from(Vec)`, добавлен `prune_prefetch` (start_current + приём Msg::Prefetch). Коммит `af7e0f3`. check/clippy/test зелёные.
- [x] (2026-11-16) M2: файловый кэш аудио — `AudioData { File, Ram }`, единый `AudioReader` (Read+Seek), `load_track_cached` с fallback в RAM, `prune_cache_dir` (защита current/queue/prefetch, лимит 8), имя файла учитывает качество. Коммит `e38764e`. check/clippy зелёные, +3 теста (15 passed).
- [x] (2026-11-16) M3: `cache-rendering-hint` в `for` лендинга гейтится по видимости — `ReleaseCard.layer-cached` (SimplePromo, NewReleases) и круг 282×282 (NewReleases); одиночные градиенты fullscreen не тронуты. Коммит `71751fe`. check/clippy/test зелёные.
- [x] (2026-11-16) M4: кэш страниц — весовой лимит 6 МБ + 80 слотов (`data_weight`, `evict`), `release_prefix` (lblock:/search: при уходе), отбрасывание результатов удалённых ключей; `cards` — LRU-кэп 24 ключа (`remember_cards`, тип `Vec<Card>` сохранён — Grid нужны полные карточки), чистка при logout; `release_page` сбрасывает results/chart/artist_tracks; `release_chart` чистит chart_tracks. Коммит `c86dcbe`. +2 теста (17 passed), clippy чисто.
- [x] (2026-11-16) M5: `ListData.tracks`/`ListPlay.all`/`Cmd::PlayList.tracks` → `Arc<Vec<Track>>` (store/UI/плеер делят буфер, убрана полная копия `list.all`); `queue_ver` в Engine/Shared — `upcoming` пересобирается только при смене очереди (убран churn 30 Track каждые 200 мс); Loop/Repeat::All расширяют очередь из Arc без промежуточного Vec. Коммит `e4ab09b`. check/clippy/test зелёные (17). Очередь остаётся `VecDeque<Track>` — полный `Arc<Track>`-повсеместно отложен (см. Decision Log).
- [x] (2026-11-16) M6: обложки — `ladder {64,128,192,320,640,900}` в `images.rs::request` и `api.rs::pic` (единая точка), BUDGET 12/8 МБ, TRIMMED 6/4; `decode` ресайзит до `to_rgba8` (без лишней полной копии); освобождение `cover_big`/`cover_view` через `changed fullscreen`/`changed cover-open` колбэки (state.slint + Rust-обработчики), `blob_src`+`App.blob` при уходе с волны. Коммит `3ccb0bd`. +assert в pic_size, check/clippy/test зелёные (17).
- [x] (2026-11-16) M7: клоны `Value` убраны — `take_result` (remove из object) в send/get_raw/wheel/session_req; `playlist` итерирует tracks по ссылке; `landing::def_one` вместо клона блока в `parse_skeleton`; `parse_block::items` возвращает `&[Value]` (правки `&it`→`it`). Коммит `92ef140`. check/clippy/test зелёные (17).
- [x] (2026-11-16) M8: убраны шрифты YSText-Light/Heavy (Black оставлен — вес 900); `drop(assets)` после `set_icons`; `artist_names: ModelRc::default()` при пустом списке; `update.rs::install` стримит чанками (Read+Write, инкрементальный SHA-256) вместо всего exe в RAM; `search.slint` — треки и чарт материализуются только по 8 (`for i in min(8, len)`). Коммит `ee42500`. check/clippy/test зелёные (17). Опциональное 900→700+Black отложено (видимое изменение).
- [x] (2026-11-16) M9: манифест `heapType=SegmentHeap` встроен через `build.rs::resources` (`1 24 "app.manifest"`, RT_MANIFEST). Коммит `e0d5301`. Локально macOS не компилирует windows-модуль — проверка сборки и замера эффекта на Windows CI/машине (эксперимент, обратим).
- [x] (2026-11-16) CI Windows зелёный (прогон 43766176971, PR #1 `rnikitin/yamusic-rs`): `clippy --release --all-targets -- -D warnings`, `cargo test --release`, `cargo build --release`, SHA-256, артефакт `yamusic-rs.exe` (27.1 МБ). Манифест SegmentHeap (M9) встроен через rc.exe без конфликта. Попутно исправлены 2 Windows-ошибки базового снимка mac-порта (не код памяти): `copy_text` (владеющий Command, коммит `20cfc8d`) и `Cmd::Pause` `cfg_attr(windows, allow(dead_code))` (коммит `3cd5240`).
- [ ] Финальный перемер памяти на Windows по сценарию README (baseline ~65 МБ → цель 30–40 МБ), заполнение таблицы в Artifacts; при подтверждении — обновление цифр в README. Выполняется пользователем на Windows.
- [ ] Повторный ребейз `memory-audit` на официальную ветку macOS-порта (когда mac-агент закоммитит), разрешение конфликтов в `player.rs`/`gui.rs`/`landing_ui.rs`, проверка компиляцией.


## Surprises & Discoveries


Этот раздел заполнен результатами аудита (фаза исследования уже проведена). Новые наблюдения добавлять по ходу реализации с короткими доказательствами.

- Observation: каждый видимый элемент `Image` в Slint+Skia хранит собственную полную копию пикселей поверх кэша приложения.
  Evidence: `i-slint-renderer-skia-1.18.1/cached_image.rs:135-141` — `skia_safe::Data::new_copy(pixels.as_bytes())`; кэш per-element (`itemrenderer.rs:349`, `ItemCache` без лимита размера). `Image::from_rgba8` даёт `ImageCacheKey::Invalid` (`i-slint-core/graphics/image.rs:842-844`), поэтому глобальный 5 МБ IMAGE_CACHE для обложек не используется. Одна обложка в N местах = N копий + 1 в кэше `Images`. Это нельзя убрать настройкой — только уменьшать число/размер копий.
- Observation: `cache-rendering-hint: true` создаёт оффскрин-слой размером с элемент на каждый экземпляр, без лимита и без выселения, пока элемент жив.
  Evidence: `i-slint-renderer-skia-1.18.1/itemrenderer.rs:972` → `render_and_blend_layer` → `render_layer` использует `layer_cache: ItemCache<Option<(PhysicalPoint, Image)>>` (`i-slint-core/item_rendering.rs:38-43` — `HashMap` без ограничения размера). В `ui/landing.slint:357` и `:440` хинт стоит на элементах внутри `for` без гейтинга по видимости.
- Observation: `Arc<[u8]>::from(Vec<u8>)` всегда делает полную копию (заголовок Arc не помещается в буфер Vec).
  Evidence: std `alloc/src/sync.rs` (~4015-4039), `ptr::copy_nonoverlapping`. В `src/player.rs:957` это третья копия файла при старте трека.
- Observation: `fetch_bytes` держит две копии файла во время скачивания.
  Evidence: `src/api.rs:1521` — `resp.bytes().map(|b| b.to_vec())`: сначала полный `Bytes`, затем копия в `Vec<u8>`.
- Observation: `Cmd::Jump` и `prev` не сбрасывают предзагрузку; чужой полный файл может жить в RAM минутами.
  Evidence: `src/player.rs:642-654` (Jump), `:871-890` (prev), `:894-915` (start_current сбрасывает только при совпадении id), `:522-537` (Msg::Prefetch принимает результат для любого id).
- Observation: `State.cards` нельзя свести к одним `target`: страница Grid использует полные `Vec<Card>` как fallback для блоков без API-загрузки.
  Evidence: `src/gui.rs:502-513` — `let cached = st.cards.get(...)` и `_ => Ok(vec![(String::new(), cached)])` в `show_grid`. Поэтому вместо замены типа на пары `(target, title)` рост `cards` ограничен LRU-кэпом 24 ключа (`remember_cards`) + очистка при logout. Это сохраняет данные для Grid и ограничивает память.
- Observation: на свежем форке GitHub Actions не регистрирует workflow, пока не переинициализировать permissions (`PUT actions/permissions enabled=false→true`); push не-`main` ветки CI не триггерит (workflow слушает `main`/PR/dispatch), а прогоны на форке выполняются последовательно (очередь), из-за чего release-сборка с холодным кэшем Skia заняла ~23 мин.
  Evidence: `gh api .../actions/workflows` total_count=0 до toggle; `gh workflow run` 404 до toggle; run 43766176971 windows job 23m2s, conclusion=success.
- Observation: базовый снимок mac-порта содержал 2 незавершённых Windows-участка, не компилирующихся под Windows (на macOS скрыты за cfg): `copy_text` (E0716 — заимствование временного Command + unused_mut) и `Cmd::Pause` (dead_code, конструируется только в `src/mac.rs`). Исправлены в ветке памяти (`20cfc8d`, `3cd5240`); при ребейзе на официальный mac-бранч возможны конфликты в этих местах.
- Observation: `reqwest` 0.12 не реэкспортирует `Bytes` в корне (`reqwest::Bytes` — ошибка E0432).
  Evidence: `cargo check` после M1. Решение: добавить `bytes = "1"` как прямую зависимость (крейт `bytes` 1.12.1 уже в дереве через reqwest) и использовать `bytes::Bytes`. Вызов `fetch_bytes` в `gui.rs` (обложка для ID3) не потребовал правок: `Option<Bytes>::as_deref()` даёт `Option<&[u8]>` благодаря `Deref<Target=[u8]>`.
- Observation: в основном дереве (`main`) и в worktree после ребейза на `mac-port-snapshot` локальная сборка на macOS проходит (`cargo check` — Finished; `cargo test` — 12 passed, 3 ignored). На чистом HEAD `45335cf` без macOS-порта были две ошибки в не-Windows ветках (`src/gui.rs::copy_text`, `src/assets.rs::asar_path`); снимок порта их закрывает.
  Evidence: `cargo check` в worktree после `git rebase mac-port-snapshot` — Finished. Это позволяет проверять изменения компиляцией локально; финальный гейт — CI Windows.


## Decision Log


- Decision: порядок майлстоунов M1→M9 выбран по соотношению «эффект / риск / объём кода»: сначала аудио (самая большая статья, изолированный модуль), затем UI-слои и кэши, затем рефакторинг очереди, затем мелочи и системный эксперимент.
  Rationale: аудио даёт −15…−20 МБ стабильно и почти не связано с остальным кодом; рефакторинг очереди (M5) затрагивает много мест и делается после того, как крупные победы уже зафиксированы.
  Date/Author: 2026-11-15, аудит + планирование.
- Decision: для аудио выбран файловый кэш в `%TEMP%` (M2), а не HTTP-range-стриминг.
  Rationale: `Api::download_to` (`src/api.rs:1503`) уже реализован и стримит на диск; rodio/symphonia требуют `Read + Seek`, что даёт `File`. Range-стриминг потребовал бы кастомный `MediaSource` и сложнее в отладке. Файловый кэш переиспользуется между сессиями и при повторе трека.
  Date/Author: 2026-11-15.
- Decision: в M2 сохраняется fallback в RAM (`Bytes`) на случай недоступного каталога кэша.
  Rationale: `%TEMP%` может быть недоступен для записи в редких окружениях; плеер не должен замолчать. Enum `AudioData { File, Ram }` сохраняет оба пути.
  Date/Author: 2026-11-15.
- Decision: бюджет кэша обложек в «плавном» режиме снижается с 16 до 12 МБ (trim 10→6), лёгкий режим не трогаем (8/4).
  Rationale: видимые обложки и так удерживаются per-item копиями Skia, поэтому кэш `Images` — преимущественно невидимый запас; снижение почти не заметно визуально, но даёт −4…−6 МБ.
  Date/Author: 2026-11-15.
- Decision: не переходить на mimalloc/jemalloc и не включать GPU-рендер.
  Rationale: mimalloc по умолчанию увеличивает commit charge (метрика Private Bytes), а GPU добавляет 100–150 МБ драйвера — это осознанно отвергнуто в README. Системный эксперимент — только Segment Heap через манифест (M9), он бесплатен по коду и обратим.
  Date/Author: 2026-11-15.
- Decision: `State.cards` хранит пары `(target, title)`, а не только `target`.
  Rationale: `open_card` (`src/gui.rs:2194`) передаёт `title` в `open_target` для поиска по тегу; один `target` недостаточен. Пары всё равно в ~4 раза меньше полных `Card`.
  Date/Author: 2026-11-15.
- Decision: коммитить только файлы, относящиеся к памяти, явным `git add <список>`; не использовать `git add -A`.
  Rationale: в рабочем дереве есть незакоммиченная работа по macOS-порту (другой поток). Её нельзя случайно включить в коммиты этого плана.
  Date/Author: 2026-11-15.
- Decision: вести работу в изолированном git worktree `memory-audit`, созданном нативным инструментом OpenCode от закоммиченного HEAD `45335cf`, а не в основном дереве.
  Rationale: пользователь запросил изоляцию от параллельной работы по macOS-порту в `main`. Worktree от чистого HEAD даёт воспроизводимую базу и не смешивает два потока изменений. План перенесён в worktree (`plan/plan-memory-optimization.md`); сессия перемещена туда.
  Date/Author: 2026-11-15.
- Decision: ребейзнуть `memory-audit` на ветку `mac-port-snapshot` (`2ac8f0c`) — автоматический снимок macOS-порта из `main`, сделанный через временный индекс (`GIT_INDEX_FILE`) без изменения HEAD/индекса/рабочего дерева `main`; свой каталог `plan/` в снимок исключён.
  Rationale: пользователь предложил подтянуть mac-сборку ребейзом, чтобы проверять изменения компиляцией локально. Достучаться до macOS-агента через Orca нельзя (в Orca нет worktree/терминала yamusic-rs), поэтому снимок сделан самостоятельно. `main` после снимка проверен — рабочее дерево и HEAD не изменились. После ребейза локальный `cargo check` проходит. Снимок WIP: при официальном коммите macOS-порта потребуется повторный ребейз на актуальную ветку (возможны конфликты в общих файлах `player.rs`, `landing_ui.rs`).
  Date/Author: 2026-11-15.
- Decision: M5 выполнен в сокращённом объёме: общий `Arc<Vec<Track>>` на границе store/UI/плеер (`ListData.tracks`, `ListPlay.all`, `Cmd::PlayList.tracks`) + `queue_ver`, но элемент очереди остаётся `Track` (`VecDeque<Track>`), а не `Arc<Track>`.
  Rationale: полный переход на `Arc<Track>` повсеместно (очередь/история/current/Shared.track/upcoming) не убирает копию очереди сам по себе — для этого нужны индексы в общий буфер или `Vec<Arc<Track>>` как каноническое хранилище, что тянет редизайн ~90 мест и runtime-проверку воспроизведения (на macOS аудио не проверить). Сокращённый вариант безопасен на уровне типов, убирает полную копию `list.all` и churn `upcoming`, сохраняя поведение. Глубокий вариант отложен как отдельная задача с runtime-тестами.
  Date/Author: 2026-11-16.
- Decision: M1/M2 (аудио-кэш на `Bytes`/файл) не переносить на ветку `macos` — там уже есть `src/stream.rs::HttpStream` (HTTP Range-стриминг), superior-замена. Лестницу размеров обложек из M6 удалить.
  Rationale: `HttpStream` стримит аудио кусками без полной загрузки в RAM — лучше файлового кэша. Лестница (`ladder`) округляла px ВВЕРХ и декодировала обложки крупнее точного размера экрана (оварсайз); выигрыш консолидации работает только для повторных артов, на уникальных обложках лендинга — регрессия. Замер: с лестницей 122 МБ, без — 120 МБ (паритет с базой 121 МБ). Оригинал декодировал ровно под размер (пиксель-оптимально). Надёжные части M6 сохранены: бюджет 12/6 МБ, ресайз до `to_rgba8`, освобождение оверлеев.
  Date/Author: 2026-11-16.


## Outcomes & Retrospective


Реализованы все майлстоуны M1–M9 (коммиты `af7e0f3`, `e38764e`, `71751fe`, `c86dcbe`, `e4ab09b`, `3ccb0bd`, `92ef140`, `ee42500`, `e0d5301`). Каждый проверен локально: `cargo check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` — зелёные (17 тестов, включая новые: очистка кэша аудио, весовое выселение кэша страниц, лестница размеров). Поведение плеера/навигации сохранено на уровне типов и логики; runtime-проверка воспроизведения и фактический замер памяти — на Windows (следующий шаг пользователя).

Что сделано по статьям памяти: аудио вынесено в файловый кэш с RAM-fallback и убраны двойные/тройные копии байтов (M1/M2); оффскрин-слои лендинга создаются только для видимых карточек (M3); кэш страниц ограничен по весу и чистится по префиксам, `cards` ограничен LRU (M4); список треков разделён через `Arc` между store/UI/плеером, `upcoming` больше не пересобирается каждые 200 мс (M5); обложки квантуются к лестнице размеров, бюджет снижен, пик декода уменьшен, картинки оверлеев освобождаются (M6); клоны `Value` на пути парсинга убраны take-паттерном (M7); лишние шрифты/спрайт/пустые модели удалены, обновление стримится (M8); добавлен манифест Segment Heap (M9, эксперимент).

Отклонения от исходного плана (зафиксированы в Decision Log): M5 выполнен в сокращённом объёме (элемент очереди остался `Track`, полный `Arc<Track>`-редизайн отложен до runtime-тестов); `cards` сохранён как `Vec<Card>` с LRU вместо пары `(target,title)` — Grid нужны полные карточки.

Уроки: (1) изолированный worktree + снимок mac-порта позволили проверять изменения компиляцией локально, не смешивая потоки; (2) `reqwest::Bytes` в 0.12 не реэкспортируется — нужен прямой `bytes`; (3) `blocking::Response` не имеет `chunk()` — стриминг через `Read`; (4) Slint `changed`-колбэки на свойствах global работают и ловят все пути закрытия оверлея.

Осталось: фактический замер памяти на Windows (оценки аудита обещают ~30–40 МБ против ~65 МБ, но это требует подтверждения), повторный ребейз на официальную ветку macOS-порта, решение по опциональному 900→700 и по Segment Heap (оставить/откатить по замеру).

### 2026-11-16: интеграция на официальную ветку `macos` и замер на macOS

macOS-агент закоммитил порт в ветку `macos` (ребейз на v1.0.2). Ветка уже содержит собственную оптимизацию аудио — `src/stream.rs::HttpStream` (HTTP Range-стриминг, куски 256 КБ, фоновая докачка, Seek, fallback на полную загрузку). Это superior-замена моих M1/M2 (файловый кэш): аудио не грузится в RAM целиком. Поэтому M1/M2 и два фикса базы (`copy_text`, `Cmd::Pause`) НЕ переносились — `macos` уже содержит `copy_text`-фикс и `#[cfg_attr(not(target_os="macos"), allow(dead_code))]` на `Pause`.

Перенесены cherry-pick'ом M3–M9 на `macos` (ветка `memory-on-macos`, коммиты `f420b48`, `8fe0d8d`, `2ca7490`, `79cae1e`, `92ec908`, `4a4a002`, `ad14c48`). Конфликты: только `player.rs` в M5 (поле `prefetch` — сохранён `HttpStream` из `macos` + добавлен `queue_ver`) и один вызов `gui.rs::add_to_playlist` (`l.tracks.to_vec()` вместо `clone()`, т.к. macos-версия принимает `Vec<Track>`). Oстальное auto-merge.

Проверки на macOS зелёные: `cargo clippy --release --all-targets -- -D warnings`, `cargo test --release` (14 passed, 5 ignored), `cargo build --release` (2m40s), `tools/make_app.sh` (бандл + ad-hoc подпись).

Контролируемое сравнение (оба бинарника из одного merge-base `f14bb87`, свежий запуск, стартовая страница Wave, устояние 60 с, `phys_footprint`): **база `macos` ≈ 121 МБ, интегрированная ветка ≈ 120 МБ** — паритет на wave-idle. Это ожидаемо: wave-idle не задействует лендинг-слои (M3), тяжёлые обложки лендинга (M6), большие списки (M4/M5) — оптимизации целят в другие состояния. macOS `phys_footprint` (~120 МБ) не сравним напрямую с Windows Private Bytes (~65 МБ README): macOS учитывает больше разделяемых страниц фреймворков. Замер confounded состоянием окна (разовый сброс базы до 88 МБ оказался аномалией backgrounding, не детерминированным поведением).

Регрессия найдена и устранена: лестница размеров M6 (`ladder`) округляла размер обложки ВВЕРХ (255→320), декодируя крупнее точного размера экрана; на уникальных обложках лендинга оварсайз вредит, консолидация помогает только повторным артам. С лестницей сборка держала 122 МБ, после удаления — 120 МБ (паритет с базой). Лестница удалена (коммит после `652e8b4`), надёжные части M6 оставлены: бюджет 12/6 МБ, ресайз до `to_rgba8`, освобождение оверлеев. Решение зафиксировано в Decision Log.

Для демонстрации эффекта M3–M9 нужен замер на обложечном лендинге («Для вас») или больших списках в одинаковом состоянии UI (требует навигации), либо авторитетный сценарий Windows CI (Private Bytes).

Windows CI на ребейзнутой ветке (`memory-audit` = macos + M3–M9, без лестницы) — **success** (прогон 43840217730, PR #1 `windows pass`): `clippy --release --all-targets -- -D warnings`, `cargo test --release`, `cargo build --release`, SHA-256 `d40274318b056538ddd126109b7251bff76561f4bdc87ca6bc8dd8871673312b`, артефакт `yamusic-rs.exe` (26 МБ). Ветка force-push'нута в форк `rnikitin/yamusic-rs` (`8ffb435`). Артефакт готов для замера памяти на Windows по сценарию README.


## Context and Orientation


Репозиторий — один бинарник `yamusic-rs` (Cargo.toml, edition 2021). Ключевые модули:

- `src/player.rs` — аудиодвижок в отдельном потоке. Структура `Engine` хранит очередь `queue: VecDeque<Track>`, историю, текущий трек `current`, байты текущего трека `current_bytes: Option<Arc<[u8]>>` и предзагрузку `prefetch: Option<(String, Vec<u8>)>`. Команды из UI приходят как `Cmd` (например `Cmd::PlayList { source, tracks: Vec<Track>, start, after }`), результаты скачивания — как `Msg::Audio`/`Msg::Prefetch`. Общее состояние для UI — `Shared` (клон целиком в `Player::state()`), синхронизируется функцией `sync()` каждые 200 мс.
- `src/api.rs` — запросы к API Яндекса. `fetch_bytes` качает файл целиком в `Vec<u8>`; `download_to` стримит на диск (используется только для скачивания треков пользователем, не плеером). `pic(uri, size)` строит URL обложки, округляя размер к доступным серверным. `QUALITY` — атомарная настройка качества (0 = 320 kbps).
- `src/net.rs` — кэш данных страниц `Store = Arc<Mutex<HashMap<String, Slot>>>`, до 80 записей, выселение по времени (LRU), но без ограничения по объёму. `Data::List(ListData)` хранит полный `Vec<Track>`.
- `src/images.rs` — кэш обложек в UI-потоке с байтовым бюджетом `BUDGET = [16 МБ, 8 МБ]` (плавный/лёгкий), `TRIMMED = [10 МБ, 4 МБ]`. Ключ `url#px`. Декодирование `decode()` сначала делает полный `to_rgba8()`, потом ресайз.
- `src/gui.rs` — связь UI с данными. `State` хранит `cards: HashMap<String, Vec<Card>>` (никогда не чистится), клоны треков (`results`, `chart`, `artist_tracks`), `blob_src` (картинка «пятна» волны). `apply_filter` строит полный `Vec<TrackItem>` для модели списка. `release_page` освобождает модели при уходе со страницы. `set_icons` нарезает SVG-спрайт; `load_assets` возвращает `Sprite` (хранит всю строку спрайта).
- `src/gui/landing_ui.rs` — страницы лендинга, хранит клоны карточек/треков блоков (`LS.cards`, `LS.tracks`, `chart_tracks`).
- `src/assets.rs` — шрифты и спрайт из `app.asar` оригинала или с веб-версии. `FONTS` регистрирует 7 шрифтов.
- `src/update.rs` — самообновление; `install` держит весь exe в памяти.
- `ui/*.slint` — интерфейс. `ui/landing.slint` — блоки лендинга; `ui/app.slint` — страницы и оверлеи (fullscreen, просмотр обложки); `ui/state.slint` — глобальное состояние и enum `Page` (wave, search, trends, concerts, nonmusic, collection, list, artist, artists, settings, grid, chart).
- `build.rs` — компиляция Slint и (на Windows) встраивание иконки/VERSIONINFO через `rc.exe` в `resources::embed()`.

Термины:

- **Private Bytes** — объём приватной закоммиченной памяти процесса в Windows; именно эта метрика используется в замерах README (снимается каждые 0,5 с в скриптовом сценарии).
- **prefetch (предзагрузка)** — скачивание следующего трека заранее, за 45 с до конца текущего, чтобы переключение было мгновенным.
- **LRU** — «вытесняй давно не использованное»: политика кэша, при которой при нехватке места удаляются самые старые записи.
- **слой (layer)** — промежуточная картинка, в которую рендерер рисует поддерево элементов, чтобы потом копировать её целиком; включается свойством `cache-rendering-hint`.
- **take-паттерн** — замена `value.clone()` на `std::mem::take(&mut value)`: содержимое перемещается без копирования, на месте остаётся пустое значение.

Текущая раскладка ~65 МБ (оценки аудита, плавный режим, сценарий README): аудиофайлы в RAM 16–20 МБ стабильно (до 30–40 в пиках); кэш обложек до 16 МБ; per-item копии Skia 3–10 МБ; слои `cache-rendering-hint` 3–15 МБ; копии списка треков 2–20+ МБ; кэш страниц 3–10 МБ; глифы и шрифты 2–8 МБ; пики декодирования и невозврат аллокатором — переменные.

Важно о состоянии репозитория и воркспейсе: работа по этому плану ведётся в изолированном git worktree `memory-audit` (каталог `/Users/rnikitin/.local/share/opencode/worktree/b66c1a/memory-audit`). Ветка создана от закоммиченного HEAD `45335cf` (1.0.0), затем ребейзнута на ветку `mac-port-snapshot` (`2ac8f0c`) — это автоматический снимок незакоммиченного macOS-порта из основного дерева, сделанный через временный индекс без изменения `main`. Благодаря этому локальные `cargo check` и `cargo test` на macOS проходят, и изменения можно проверять компиляцией по ходу. Основное дерево (`/Users/rnikitin/dev/yamusic-rs`, ветка `main`) содержит живой macOS-порт (другой поток работы, файлы `PLAN.md`, `src/mac.rs`, `src/win/`, `src/tray/` и правки `player.rs`, `landing_ui.rs` и др.) — этот план его не трогает. `mac-port-snapshot` — WIP-снимок: когда macOS-агент закоммитит свою работу официально, ветку `memory-audit` нужно будет ребейзнуть на его актуальную ветку (возможны конфликты в `player.rs`/`landing_ui.rs`, которые трогают оба потока). Основная финальная валидация — CI на Windows (`.github/workflows/build.yml`) и замер памяти на Windows; локальная macOS-сборка — вспомогательная проверка компиляции.


## Plan of Work


Работа разбита на майлстоуны M0–M9. Каждый независимо проверяем и коммитится отдельно. Ниже — суть каждого; точные команды и критерии приёмки — в разделах «Concrete Steps» и «Validation and Acceptance».

**M0 — база.** Работа ведётся в worktree `memory-audit`, ребейзнутом на `mac-port-snapshot` (`2ac8f0c`, снимок macOS-порта); план в `plan/`. Локальные `cargo check` и `cargo test` на macOS проходят (подтверждено). CI на Windows (`clippy/test/build`) — финальный гейт сборки. Снять базовый замер памяти на Windows по сценарию README и записать число в `Artifacts and Notes`. Ничего в коде не менять.

**M1 — аудио: убрать лишние копии байтов.** В `src/api.rs` функция `fetch_bytes` возвращает `Res<reqwest::Bytes>` вместо `Res<Vec<u8>>` (убрать `.to_vec()`). В `src/player.rs`: `load_track`, `Msg::Audio`, `Msg::Prefetch`, `play_loaded`, поля `current_bytes` и `prefetch` переходят на `Bytes`; `Cursor<Bytes>` валиден для `Decoder::new` (клон `Bytes` — это инкремент счётчика, не копия). Добавить сброс осиротевшей предзагрузки: вспомогательная функция `prune_prefetch`, вызываемая в `Cmd::Jump`, `prev`, `start_current` и при приёме `Msg::Prefetch`, — она удаляет `prefetch`, если его id не совпадает ни с текущим треком, ни с головой очереди. Ожидаемо: −10…−18 МБ к пикам, поведение не меняется.

**M2 — аудио: файловый кэш.** Ввести enum `AudioData { File(PathBuf), Ram(Bytes) }`. Новая функция `load_track_cached(api, t) -> Res<AudioData>`: каталог `std::env::temp_dir().join("yamusic-rs")`, имя файла `<id>-q<quality>.mp3` (учитывает качество, чтобы смена качества не отдавала старый файл); если файл есть и непустой — вернуть `File`; иначе `api.download_to(url, tmp)` + переименование; при любой ошибке кэша — fallback `Ram(fetch_bytes)`. После успешного скачивания чистить каталог: защищать файлы текущего трека, головы очереди и prefetch, остальные удалять сверх лимита 8 файлов (по времени изменения). `Engine.current_bytes` → `current: Option<AudioData>` (имя поля можно оставить `current_bytes`), `prefetch: Option<(String, AudioData)>`. `play_loaded` открывает `Decoder::new(BufReader::new(File::open(path)))` для `File` и `Decoder::new(Cursor::new(bytes))` для `Ram`; `replay()` переоткрывает файл. Ожидаемо: −15…−20 МБ стабильно; на диске в `%TEMP%` до ~80 МБ (8 файлов), каталог per-user.

**M3 — слои лендинга.** В `ui/landing.slint` у компонента `ReleaseCard` (строка ~350) заменить `cache-rendering-hint: App.motion;` на `cache-rendering-hint: layer-cached;` и добавить `in property <bool> layer-cached: true;`. В местах инстанцирования (`SimplePromo` ~408 и `NewReleases` ~457) передавать видимость: для `SimplePromo` — `layer-cached: App.motion && (strip.shift + i * (root.w + 16px) + root.w > 0px && strip.shift + i * (root.w + 16px) < root.width);`, для `NewReleases` — аналогично с шагом `356px` и шириной `340px`. У круга 282×282 внутри `for` в `NewReleases` (~440) заменить хинт на инлайн-выражение видимости относительно `root.width`. Хинты в `ui/fullscreen.slint:452,458` (одиночные градиенты) оставить. Ожидаемо: −3…−15 МБ в плавном режиме на лендинге; CPU при прокрутке видимых карточек не должен вырасти заметно.

**M4 — кэш страниц и карточки.** В `src/net.rs`: добавить в `Slot` поле `weight: usize` и функцию `data_weight(&Data) -> usize` (оценка байт: число треков/карточек × ~800); при вставке результата считать суммарный вес и выселять старейшие незагружающиеся записи, пока сумма > 6_000_000 (лимит по количеству 80 оставить как вторичный). Добавить `pub fn release_prefix(&self, prefix: &str)` — удаляет ключи с префиксом; в `load()` при завершении, если ключ уже удалён, результат отбрасывать (заменить `st.entry(key).or_insert(...)` на проверку наличия). В `src/gui.rs`: `State.cards` → `HashMap<String, Vec<(String, String)>>` (target, title); `card_model` и `landing_header` кладут пары; `open_card` читает пару. В `release_page` вызывать `net.release_prefix("lblock:")` для лендинг-страниц и `net.release_prefix("search:")` для поиска; очищать `st.results`, `st.chart`, `st.artist_tracks`, `st.queue_ids`; в ветке `Page::Grid` удалять `cards["grid"]`; в `logout` — `cards.clear()`; в `release_chart` (`landing_ui.rs`) — `chart_tracks.clear()`. Ожидаемо: −5…−20 МБ при активной навигации.

**M5 — очередь на Arc.** `net.rs`: `ListData.tracks: Arc<Vec<Track>>` (оборачивать в `fetch` и `play_artist`). `player.rs`: `Cmd::PlayList.tracks: Arc<Vec<Track>>`, `ListPlay.all: Arc<Vec<Track>>`, `queue: VecDeque<Arc<Track>>`, `history: Vec<Arc<Track>>`, `current: Option<Arc<Track>>`, `Shared.track: Option<Arc<Track>>`, `Shared.upcoming: Vec<Arc<Track>>`. Добавить `queue_ver: u64` в `Engine` и `Shared`, инкрементировать при каждом изменении очереди/текущего трека; в `sync()` пересобирать `upcoming` только при смене версии. Обновить всех отправителей `Cmd::PlayList` (`gui.rs::play_list`, `net.rs::play_artist`, превью лайков, лендинг) — компилятор укажет места. Волновые пакеты: `queue.extend(fresh.into_iter().map(Arc::new))`. Ожидаемо: −2…−20 МБ на больших списках, исчезают полные клоны при Loop/Repeat/Shuffle и churn `upcoming` каждые 200 мс.

**M6 — обложки.** В `src/images.rs` добавить `pub fn ladder(px: u32) -> u32` с лестницей `{64, 128, 192, 320, 640, 900}` (первое ≥ px, иначе 900); применять в `request()` для ключа и декода. В `src/api.rs::pic()` квантовать `size` той же лестницей до серверного округления — единая точка для всех URL обложек. Снизить `BUDGET` до `[12<<20, 8<<20]`, `TRIMMED` до `[6<<20, 4<<20]`. В `decode()` ресайзить до `to_rgba8()` (убрать лишнюю полную копию). Освобождение оверлеев: в `gui.rs` ввести helper, который при закрытии fullscreen ставит `App.cover-big = Image::default()` (заменить существующие `set_fullscreen(false)` в обработчиках закрытия); для просмотра обложки добавить в `ui/app.slint` callback `cover-closed` и `changed cover-open => { if (!cover-open) { cover-closed(); } }`, в Rust очищать `App.cover-view`; при уходе со страницы волны (`release_page`, ветка `Page::Wave`) очищать `st.blob_src` и `App.blob`. Ожидаемо: −4…−10 МБ.

**M7 — клоны Value.** В `src/api.rs` заменить `.get("result").cloned()` на take-паттерн в `send()` (~855), `get_raw()` (~892), `wheel()` (~1101), `session_req()` (~1157): `match v.get_mut("result") { Some(r) => std::mem::take(r), None => ... }` (сохранить семантику fallback: `Null` в `send`, `v` в остальных). В `playlist()` (~1314) итерировать `v.get("tracks").and_then(as_array)` по ссылке без `.cloned()`. В `src/landing.rs`: `parse_block` (~326) — замыкание `items` возвращает `&[Value]` вместо `Vec<Value>`; `parse_skeleton` (~113) — выделить `defs_one(b: &Value)` и вызывать её вместо `defs(&Value::Array(vec![b.clone()]))`. Ожидаемо: −5…−15 МБ к пикам при открытии больших страниц.

**M8 — мелочи.** `src/assets.rs`: убрать из `FONTS` неиспользуемые `YSText-Light` и `YSText-Heavy` (в UI используются веса 400/500/700/900). `src/gui.rs`: после `set_icons` сделать `drop(assets)` (спрайт больше не нужен; шрифты регистрируются внутри `load_assets`); в `track_item` при пустом `artist_list` ставить `artist_names: ModelRc::default()`. `src/update.rs::install`: стримить ответ чанками в файл с инкрементальным `Sha256` вместо `bytes` целиком. `ui/search.slint` (~95): `for i in min(8, App.sr-tracks.length)` с `item: App.sr-tracks[i]` вместо материализации всех строк. Опционально (видимое изменение): `ui/landing.slint:917` вес 900→700 и убрать `YSText-Black` — решать по результату визуальной сверки. Ожидаемо: −1.5…−4 МБ.

**M9 — Segment Heap и финал.** В `build.rs::resources::embed()` добавить манифест с `<heapType>SegmentHeap</heapType>` (Windows 10 2004+; на старых ОС элемент игнорируется) и подключить его в `.rc` как `1 24 "app.manifest"`. Перемерить память; если выигрыш есть — оставить, иначе откатить. Снять финальный замер по сценарию README, записать в `Outcomes & Retrospective`; при подтверждённом результате обновить цифры в `README.md` (только после реального замера, не по оценкам).


## Concrete Steps


Все команды выполняются из корня репозитория `/Users/rnikitin/dev/yamusic-rs`, если не указано иное.

M0 (worktree создан и ребейзнут на `mac-port-snapshot`, ветка `memory-audit` активна; команды — из корня worktree):

    git branch --show-current   # ожидается: memory-audit
    cargo check                 # ожидается: Finished (macOS-порт в базе)
    cargo test                  # ожидается: 12 passed; 0 failed; 3 ignored

Финальный гейт сборки — CI на Windows (push ветки запускает `.github/workflows/build.yml`). Базовый замер памяти на Windows (PowerShell, после прогона сценария из README):

    Get-Process yamusic-rs | Select-Object @{n='PrivateMB';e={[math]::Round($_.PrivateMemorySize64/1MB,1)}}

Записать полученное число в `Artifacts and Notes` как baseline.

После каждого майлстоуна M1–M8:

    cargo check
    cargo test
    cargo clippy --all-targets -- -D warnings

Ожидается: без ошибок и предупреждений clippy (CI использует `--release`, локально допустим dev). Затем коммит только затронутых файлов, например для M1:

    git add src/api.rs src/player.rs
    git commit -m "memory: аудио без лишних копий байтов (M1)"

Для проверки на Windows pushes в ветку запускают CI (`.github/workflows/build.yml`: clippy, test, build на `windows-latest`). Финальный exe для замера памяти брать из артефактов CI или собирать на Windows: `cargo build --release`.

Замер памяти после каждого крупного майлстоуна (M1, M2, M3, M4, M5, M6) — тот же сценарий README и та же команда PowerShell; дельту записывать в `Progress` и `Artifacts and Notes`.


## Validation and Acceptance


Общая приёмка для всех майлстоунов: локально в worktree `cargo check`, `cargo test` (12 passed) и `cargo clippy --all-targets -- -D warnings` проходят; CI на Windows зелёный (`clippy --release`, `test --release`, `build --release`). Поведенческие проверки (выполнять на Windows с реальным аккаунтом, качество 320 kbps):

- M1/M2: трек играет; перемотка в середину работает; Repeat::One повторяет трек с начала; Next/Prev переключают; прыжок по очереди (Jump) переключает мгновенно, если prefetch готов; за 45 с до конца следующий трек предзагружается (в M2 — файл появляется в `%TEMP%\yamusic-rs`); смена качества в настройках приводит к скачиванию файла нового качества, а не воспроизведению старого; Logout останавливает звук и очищает очередь. В M2 при недоступном `%TEMP%` (проверить, задав TEMP несуществующим путём) трек всё равно играет через RAM-fallback.
- M3: на странице «Для вас и Тренды» в плавном режиме прокрутка блоков «Новые релизы» и промо остаётся плавной; память на этой странице ниже baseline (замер до/после прокрутки вниз).
- M4: открыть «Мне нравится» (большой список), несколько плейлистов, поиск по разным запросам, лендинги; уйти со страниц и вернуться — данные подгружаются заново (допустим краткий индикатор загрузки), приложение не теряет состояние навигации; после долгой сессии память не растёт монотонно.
- M5: играть большой плейлист и «Мне нравится»; Shuffle вкл/выкл, Repeat::All по кругу, «играть следующим» — порядок треков корректен; очередь в UI показывает ближайшие 30; память при играющем большом списке ниже, чем до M5.
- M6: обложки в списке, плеер-баре, волне, fullscreen и просмотре выглядят чётко (без заметной мыльности); закрыть fullscreen и просмотр обложки — память освобождается (замер); уйти с волны — «пятно» очищается.
- M7: открыть большой плейлист и лендинг — данные отображаются полностью и корректно (названия, обложки, треки); пик памяти при открытии ниже.
- M8: шрифты заголовков и текста выглядят как раньше (веса 400/500/700/900 на месте); иконки все на месте; кнопка «Обновить» скачивает и устанавливает обновление (проверить на тестовом релизе или хотя бы что загрузка идёт чанками и SHA-256 совпадает); поиск показывает 8 треков как раньше.
- M9: приложение запускается на Windows 10 2004+ и на более старой ОС (манифест не ломает запуск); замер памяти до/после включения Segment Heap.

Финальная приёмка: в сценарии README (играет трек, открыт текст песни) Private Bytes ≈ 30–40 МБ против baseline ~65 МБ. Если фактический результат отличается — записать реальные числа и разбор в `Outcomes & Retrospective`, не подгонять README под оценки.


## Idempotence and Recovery


Все шаги повторяемы. Команды сборки и тестов не меняют состояние. Файловый кэш M2 создаётся по требованию (`create_dir_all`), повторные запуски переиспользуют файлы; лимит 8 файлов не даёт каталогу расти бесконечно. При повреждённом/нулевом файле кэша он перезакачивается. Откат любого майлстоуна — `git revert <commit>`; так как коммиты изолированы по файлам, откат не затрагивает macOS-порт. M9 (манифест) откатывается одним коммитом, если выигрыша нет. Не использовать `git add -A` и не коммитить файлы macOS-порта (`src/win/`, `src/tray/`, `PLAN.md` и связанные изменения) — они принадлежат другой работе.


## Artifacts and Notes


Шаблон таблицы замеров (заполнять по ходу; baseline снять в M0):

    Майлстоун | PrivateMB (сценарий README) | Дельта
    baseline  | ~65 (подтвердить)           | —
    M1        |                             |
    M2        |                             |
    M3        |                             |
    M4        |                             |
    M5        |                             |
    M6        |                             |
    M9        |                             |

Ключевой фрагмент M1 (api.rs):

    pub fn fetch_bytes(&self, url: &str) -> Res<Bytes> {
        let resp = self.http.get(url).send().map_err(|e| format!("сеть: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }
        resp.bytes().map_err(|e| format!("сеть: {e}"))
    }

Ключевой фрагмент M7 (take-паттерн, send):

    let mut v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(match v.get_mut("result") {
        Some(r) => std::mem::take(r),
        None => Value::Null,
    })

Манифест M9 (app.manifest, вкладывается в exe через rc):

    <?xml version="1.0" encoding="UTF-8" standalone="yes"?>
    <assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0"
              xmlns:asmv3="urn:schemas-microsoft-com:asm.v3">
      <asmv3:application>
        <asmv3:windowsSettings xmlns="http://schemas.microsoft.com/SMI/2020/WindowsSettings">
          <heapType>SegmentHeap</heapType>
        </asmv3:windowsSettings>
      </asmv3:application>
    </assembly>

В `app.rc` добавить строку `1 24 "app.manifest"` (24 = RT_MANIFEST), файл манифеста копировать в `OUT_DIR` рядом с `app.ico`.


## Interfaces and Dependencies


Добавлена одна прямая зависимость `bytes = "1"` (крейт уже был в дереве через reqwest; `reqwest::Bytes` в 0.12 не реэкспортируется). `bytes::Bytes` реализует `AsRef<[u8]>`, `Deref<Target=[u8]>`, `Send`, `'static`, клон дешёвый (refcount). `Cursor<Bytes>` и `BufReader<File>` удовлетворяют `Read + Seek`, которые требует `rodio::Decoder::new`.

В `src/images.rs` определить:

    pub fn ladder(px: u32) -> u32 {
        [64, 128, 192, 320, 640, 900].into_iter().find(|&s| s >= px).unwrap_or(900)
    }

В `src/player.rs` определить:

    enum AudioData { File(std::path::PathBuf), Ram(Bytes) }

и изменить сигнатуры: `fn load_track_cached(api: &Api, t: &Track) -> Res<AudioData>`, `fn play_loaded(&mut self, data: AudioData)`, `fn prune_prefetch(&mut self)`; поля `Engine { current_bytes: Option<AudioData>, prefetch: Option<(String, AudioData)>, queue: VecDeque<Arc<Track>>, history: Vec<Arc<Track>>, current: Option<Arc<Track>>, queue_ver: u64 }`; `Cmd::PlayList { tracks: Arc<Vec<Track>>, .. }`; `Shared { track: Option<Arc<Track>>, upcoming: Vec<Arc<Track>>, queue_ver: u64, .. }`.

В `src/net.rs` определить:

    fn data_weight(d: &Data) -> usize;
    impl Net { pub fn release_prefix(&self, prefix: &str); }

и изменить `ListData { tracks: Arc<Vec<Track>>, .. }`, `Slot { weight: usize, .. }`.

В `src/api.rs`: `pub fn fetch_bytes(&self, url: &str) -> Res<Bytes>`; `pic()` применяет `crate::images::ladder` к размеру до серверного округления.

В `src/gui.rs`: `State { cards: HashMap<String, Vec<Card>>, cards_order: Vec<String>, .. }` (LRU-кэп 24 через `remember_cards`); `changed fullscreen`/`changed cover-open` колбэки (state.slint) с Rust-обработчиками, очищающими `App.cover-big`/`App.cover-view`; `release_page` чистит `blob_src` на Wave и вызывает `net.release_prefix`.

Существующие контракты, которые нельзя сломать: `Player::state() -> Shared` (UI читает поля), `Images::get/get_cancellable` (колбэк с `&Image`), `Net::get/peek/refresh`, формат ключей кэша (`album:`, `playlist:uid:kind`, `likes`, `search:all:`, `lblock:kind|uri`, `lyrics:`, `skeleton:`), сериализация `Config`, протокол волны (`wave_session`, фидбеки).


## Revision Notes


- 2026-11-16: план выполнен (M1–M9). Обновлены Progress, Surprises & Discoveries, Decision Log, Outcomes & Retrospective и Interfaces под фактическую реализацию. Ключевые отклонения от исходного замысла: M5 сокращён до общего `Arc<Vec<Track>>` + `queue_ver` (элемент очереди остался `Track`); `cards` сохранён как `Vec<Card>` с LRU-кэпом вместо пар `(target, title)` — страница Grid использует полные карточки как fallback (`gui.rs::show_grid`). Причина обоих — снижение риска без runtime-проверки аудио и сохранение поведения Grid. Добавлены прямая зависимость `bytes = "1"` и заметки о `reqwest::Bytes`/`blocking::Response::chunk`. Работа велась в worktree `memory-audit`, ребейзнутом на снимок macOS-порта `mac-port-snapshot`.
