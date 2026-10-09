# План: code-editor — opensource IDE на Rust для PHP/JS/TS

## Context

Нужна собственная IDE уровня «JetBrains-lite»: редактор с подсветкой, git через GUI, подключение к Postgres/Redis с read-only режимом, индекс кодовой базы в SQLite, JetBrains-хоткеи, Material-тема, автосохранение, панель Docker. Проект opensource (MIT), на Rust ради памяти и перфоманса. Проекта ещё нет — всё с нуля.

## Решения по итогам опроса

| Вопрос | Решение |
|---|---|
| Путь | `~/Desktop/Projects/code-editor` |
| GUI | `egui`/`eframe` 0.36 (чистый Rust, wgpu) |
| Платформа | только macOS (Cmd-хоткеи), кросс-платформа позже |
| Rust | ставлю через `rustup` (stable) на старте реализации |
| Git | вызов системного `git` CLI, парсинг porcelain-вывода |
| DB read-only | два слоя: серверный (`default_transaction_read_only`) + клиентская блокировка DML/DDL с диалогом |
| Языки | PHP, JS, TS/TSX + JSON/HTML/CSS/Markdown/YAML/SQL (tree-sitter) |
| Code intel | LSP-клиент: `intelephense` (PHP), `typescript-language-server` (JS/TS) через node |
| Docker | контейнеры, образы, логи, exec, compose up/down/ps (`bollard` + `docker compose` CLI) |
| Тема | Material Palenight по умолчанию, Oceanic/Darker/Lighter переключаемые |
| Индекс | SQLite: файлы + FTS5 полный текст + символы из tree-sitter |
| Терминал | встроенный, `portable-pty` + `alacritty_terminal` |

## Архитектура

Один бинарный крейт, модули по доменам. Без workspace — разбить на крейты, когда время компиляции станет болеть.

```
code-editor/
├── Cargo.toml
├── LICENSE                 (MIT)
├── README.md
├── .gitignore
├── assets/
│   ├── fonts/              JetBrains Mono (OFL) — моноширинный шрифт редактора
│   ├── queries/            highlights.scm на каждый язык (копии из tree-sitter-* репо)
│   └── themes/*.toml       palenight, oceanic, darker, lighter
└── src/
    ├── main.rs             eframe::run_native, инициализация tokio runtime, логирование
    ├── app.rs              App { workspace, panes, settings } — корневой egui::App, раскладка панелей, роутинг хоткеев
    ├── keymap.rs           таблица JetBrains-хоткеев → Action enum
    ├── theme.rs            загрузка .toml → egui::Visuals + цвета подсветки
    ├── settings.rs         ~/Library/Application Support/code-editor/settings.toml (serde)
    ├── editor/
    │   ├── buffer.rs       ropey::Rope + undo/redo + dirty-флаг + автосохранение
    │   ├── view.rs         egui-виджет: gutter, курсор, выделение, скролл, рендер подсветки
    │   ├── highlight.rs    tree-sitter-highlight → spans по строкам, инкрементальный reparse
    │   └── languages.rs    расширение файла → Language + highlights.scm
    ├── workspace/
    │   ├── tree.rs         файловое дерево (панель Project), уважает .gitignore через `ignore`
    │   └── watcher.rs      notify → события для индекса и дерева
    ├── index/
    │   ├── db.rs           rusqlite, схема, миграции (SQLite в .code-editor/index.db внутри проекта)
    │   ├── indexer.rs      фоновая задача: обход файлов, FTS5, извлечение символов tree-sitter
    │   └── search.rs       запросы: файлы (fuzzy), текст (FTS5), символы
    ├── search_ui.rs        Shift+Shift / Cmd+Shift+O / Cmd+Shift+F / Cmd+F попапы
    ├── lsp/
    │   ├── client.rs       JSON-RPC по stdio, lsp-types, запуск/перезапуск серверов
    │   └── features.rs     completion, hover, goto-def, diagnostics → в редактор
    ├── git/
    │   ├── cmd.rs          run_git(args) -> Result<Output>, парсинг `status --porcelain=v2`, `log --format`, diff
    │   └── ui.rs           панель Git: changes, commit, log, branches, diff-вьюер, диалоги (merge/rebase/stash/push/pull)
    ├── db/
    │   ├── postgres.rs     tokio-postgres, read-only сессия, выполнение SQL, результат → таблица
    │   ├── redis.rs        redis crate, read-only блок-лист команд
    │   ├── guard.rs        классификация запроса: sqlparser для PG, список команд для Redis → ReadOnlyViolation
    │   └── ui.rs          панель Database: соединения, консоль, таблица результата, схема (PG), ключи (Redis)
    ├── docker/
    │   ├── api.rs          bollard: контейнеры, образы, логи (stream), exec
    │   ├── compose.rs      `docker compose` CLI
    │   └── ui.rs           панель Services
    └── terminal/
        ├── pty.rs          portable-pty, чтение/запись в фоне
        └── view.rs         alacritty_terminal grid → egui рендер
```

Фоновые операции (git, БД, Docker, индекс, LSP) идут через `tokio` runtime в отдельных задачах, результаты в UI через `std::sync::mpsc` + `ctx.request_repaint()`. UI-поток не блокируется.

## Зависимости (версии проверены на crates.io 2026-10-09)

| Крейт | Версия | Зачем |
|---|---|---|
| eframe, egui, egui_extras | 0.36 | GUI, таблицы, шрифты |
| ropey | 1.6 | текстовый буфер |
| tree-sitter, tree-sitter-highlight | 0.27 | парсинг + подсветка |
| tree-sitter-php 0.25, -javascript 0.25, -typescript 0.23, + json/html/css/md/yaml/sql | | грамматики (при несовместимости ABI — пин к общей версии tree-sitter) |
| rusqlite (bundled, fts5) | 0.40 | индекс |
| ignore, notify | 0.4 / 8 | обход ФС с .gitignore, watcher |
| tokio (rt-multi-thread, process, io) | 1.53 | async runtime, процессы |
| tokio-postgres | 0.7 | Postgres |
| redis (tokio-comp) | 1.7 | Redis |
| sqlparser | 0.63 | классификация SQL для read-only guard |
| bollard | 0.21 | Docker API |
| portable-pty, alacritty_terminal | 0.9 / 0.26 | терминал |
| lsp-types, serde_json | 0.97 | LSP |
| serde, toml, directories, rfd, anyhow, tracing | | настройки, диалоги, ошибки, логи |

## Хоткеи (JetBrains macOS keymap)

| Хоткей | Действие |
|---|---|
| Shift Shift | Search Everywhere (файлы + символы + действия) |
| Cmd+Shift+O | Go to File |
| Cmd+O | Go to Class |
| Cmd+Alt+O | Go to Symbol |
| Cmd+Shift+A | Find Action |
| Cmd+F / Cmd+R | Find / Replace в файле |
| Cmd+Shift+F / Cmd+Shift+R | Find / Replace in Path |
| Cmd+B | Go to Definition (LSP) |
| Ctrl+Space | Completion |
| Cmd+/ | Toggle comment |
| Cmd+D | Duplicate line |
| Cmd+Backspace | Delete line |
| Alt+Up/Down | Expand/shrink selection |
| Cmd+Shift+Up/Down | Move line |
| Cmd+K / Cmd+Shift+K | Commit / Push |
| Cmd+T | Update project (pull) |
| Cmd+1 / Cmd+9 / Cmd+5 | Project / Git / Debug(Database) панели |
| Alt+F12 | Terminal |
| Cmd+E | Recent files |
| Cmd+S | Сохранить сейчас (автосохранение и так включено) |
| Esc | Закрыть попап / фокус в редактор |

Таблица в `keymap.rs` как `&[(Modifiers, Key, Action)]`, переопределяется из settings.toml.

## Read-only режим БД (как в JetBrains)

- Флаг `read_only: bool` в настройках соединения, переключатель и иконка замка в панели.
- **Postgres:** при коннекте `SET default_transaction_read_only = ON`; перед выполнением `sqlparser` классифицирует statement: `Query`/`Explain`/`Show` — ok; `Insert/Update/Delete/Create/Alter/Drop/Truncate/Grant/...` — диалог «Соединение в режиме read-only. Выполнить всё равно?» → при согласии выполняем в транзакции с `SET TRANSACTION READ WRITE`.
- **Redis:** блок-лист пишущих команд (SET, DEL, FLUSHALL, FLUSHDB, EXPIRE, HSET, LPUSH, SADD, ZADD, INCR, CONFIG SET, … — берётся из `COMMAND INFO` флага `write` при коннекте, фолбэк статический список). Тот же диалог.
- Пароли — в macOS Keychain через `security` CLI (без доп. крейта); в settings только хост/порт/юзер/db.

## Индекс SQLite

```sql
files(id, path UNIQUE, lang, mtime, size)
files_fts  -- FTS5(content, path UNINDEXED), tokenize='trigram'  → поиск подстрок и регистронезависимо
symbols(id, file_id, name, kind, line, col)  -- kind: class|function|method|const|interface|trait
CREATE INDEX symbols_name ON symbols(name COLLATE NOCASE);
```

- Первичная индексация в фоне при открытии проекта, прогресс в статус-баре.
- Инкрементально по событиям `notify` + сверка mtime при старте.
- Символы извлекаются tree-sitter-запросами (`tags.scm`-подобные) для PHP/JS/TS.
- Fuzzy-ранжирование для Go to File — простая subsequence-оценка в `search.rs`, без крейта.

## Автосохранение

Буфер пишется на диск через 1 с после последнего изменения (debounce) и при потере фокуса окна. Атомарно: запись во временный файл + rename. В `settings.toml`: `autosave = true`, `autosave_delay_ms = 1000`.

## Тема

`assets/themes/palenight.toml`: цвета UI (bg, panel, accent, selection, …) + токены подсветки (keyword, string, function, type, comment, …) по палитре Material Theme. `theme.rs` маппит в `egui::Visuals` и в `HashMap<highlight_name, Color32>`. Шрифт JetBrains Mono вшит через `include_bytes!`.

## Фазы реализации

Каждая фаза заканчивается запускаемым приложением и коммитом. **Статус (2026-10-09): фазы 0–8 реализованы**, см. `git log`.

**Фаза 0 — Bootstrap.** rustup, `cargo new`, MIT LICENSE, README, .gitignore, `git init` + первый коммит. Пустое eframe-окно с темой Palenight и шрифтом. Проверка: `cargo run` открывает окно.

**Фаза 1 — Редактор.** Открыть папку (rfd), дерево файлов, вкладки, буфер ropey, ввод/выделение/undo, подсветка tree-sitter для всех языков, gutter с номерами, автосохранение. Хоткеи: Cmd+F, Cmd+/, Cmd+D, Cmd+Backspace, Cmd+S, Cmd+E, move line. Проверка: открыть `~/Desktop/Projects/crm`, редактировать PHP и JS, подсветка корректна, файл сохраняется сам.

**Фаза 2 — Индекс и поиск.** SQLite-индекс, watcher, Shift+Shift, Cmd+Shift+O, Cmd+O, Cmd+Alt+O, Cmd+Shift+F с результатами и переходом. Проверка: на `crm` индексация < 10 с, поиск класса находит файл, изменение файла обновляет индекс.

**Фаза 3 — Git.** Панель Changes (status), stage/unstage, diff-вьюер (unified, по hunks), commit (Cmd+K), log с графом веток, branches (create/checkout/merge/delete), push/pull/fetch, stash, rebase (non-interactive + continue/abort), reset, cherry-pick, blame в gutter, индикатор изменённых строк. Проверка: полный цикл branch → edit → commit → merge на тестовом репо.

**Фаза 4 — Терминал.** PTY-панель Alt+F12, несколько вкладок, запуск в корне проекта. Проверка: `ls`, `vim`, `htop` рендерятся.

**Фаза 5 — LSP.** Запуск intelephense и typescript-language-server (npx или глобально, путь в настройках), completion (Ctrl+Space), hover, Cmd+B, diagnostics подчёркиванием. Проверка: автодополнение метода класса в PHP и импорта в TS.

**Фаза 6 — Database.** Менеджер соединений, read-only guard (оба слоя), SQL-консоль с подсветкой, таблица результатов (egui_extras::Table, пагинация), обзор схемы PG (schemas/tables/columns), обзор ключей Redis (SCAN, типы, значение). Проверка: в read-only `UPDATE` показывает диалог и отклоняется; `SELECT` работает; Redis `SET` блокируется, `GET` работает (на локальных docker-контейнерах PG и Redis).

**Фаза 7 — Docker.** Панель Services: контейнеры (list/start/stop/restart/rm, статус, порты), образы (list/pull/rm), логи со стримом и фильтром, exec в терминал-вкладку, compose up/down/ps для найденных `docker-compose.yml`. Проверка: поднять PG+Redis из compose внутри IDE, подключиться к ним из фазы 6.

**Фаза 8 — Полировка.** Переключение тем, settings UI, recent projects, `cargo build --release` + `.app` bundle через `cargo-bundle`, README с скриншотами.

## Сознательные упрощения (ponytail)

- Позиции LSP считаются в char-индексах, а не UTF-16 (расходится только на не-BMP символах в строке).
- Полная переподсветка файла на каждое изменение; инкрементальный tree-sitter — когда файлы > 1 МБ начнут тормозить.
- Нет merge-tool для конфликтов: файлы с конфликтом помечены, правятся в редакторе.
- Redis read-only: статический список пишущих команд.

- Один крейт, не workspace — разбивать, когда `cargo build` станет > 1 мин инкрементально.
- Git через CLI — без libgit2; если понадобится работа без установленного git, добавим `git2`.
- Без плагинной системы, без debugger (xdebug), без refactoring — не запрашивались.
- Fuzzy-поиск — свой subsequence-скорер, не `nucleo`; заменить, если ранжирование окажется плохим.
- LSP-серверы не бандлятся — требуют node, который на машине есть.

## Верификация

- `cargo build` и `cargo clippy` без warnings на каждой фазе.
- Юнит-тесты только для логики с ветвлением: парсинг `git status --porcelain=v2`, классификатор read-only (`guard.rs`), fuzzy-скорер, debounce автосохранения, извлечение символов.
- Ручная проверка по чек-листам фаз на реальных проектах из `~/Desktop/Projects` и на docker-контейнерах PG/Redis.
