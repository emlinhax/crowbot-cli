Read a file (or list a directory) from the local filesystem.

- `path` may be absolute or relative to the working directory.
- Output is numbered `cat -n` style. Line numbers are for reference only; never include them in edits.
- At most {max_lines} lines are returned; pass `offset` (1-based line) and `limit` to page through longer files.
- Read a file before editing or overwriting it.
