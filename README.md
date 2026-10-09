# code-editor

Opensource IDE на Rust для PHP / JavaScript / TypeScript.

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
cargo run --release
```

Лицензия: MIT.
