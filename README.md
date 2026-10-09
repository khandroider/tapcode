# code-editor

Opensource IDE на Rust для PHP / JavaScript / TypeScript.

![screenshot](docs/screenshot.png)

- Подсветка синтаксиса через tree-sitter, тема Material Palenight
- Git через GUI (JetBrains-style)
- Подключение к Postgres и Redis с read-only режимом
- Индекс кодовой базы в SQLite (файлы, полный текст, символы)
- JetBrains-хоткеи (Shift+Shift, Cmd+Shift+O, Cmd+Shift+F, …)
- Автосохранение по умолчанию
- Панель Docker (контейнеры, образы, логи, compose)
- Встроенный терминал, LSP (intelephense, typescript-language-server)

## Сборка

```bash
cargo run --release -- ~/path/to/project
```

Переменная `CODE_EDITOR_SCREENSHOT=out.png` снимает окно через секунду и выходит (для README и smoke-проверок).

Лицензия: MIT.
