Search file contents with a regular expression (ripgrep syntax). Respects .gitignore.

- Returns `path:line: text` for each match, at most {max_results}.
- Narrow with `glob` (e.g. `*.ts`) or `path`; use `ignore_case` for case-insensitive search.
