Read files, or list a directory, from the local filesystem.

- `path` may be absolute or relative to the working directory. Pass a list of up to {read_batch_max} paths to read several files in one call, each under its own name; `offset` and `limit` then apply to each.
- Output is numbered `cat -n` style. Line numbers are for reference only; never include them in edits.
- At most {max_lines} lines are returned; pass `offset` (1-based line) and `limit` to page through longer files.
- A directory lists its entries. `depth` goes that many levels down (at most {list_depth_max}), indenting each; levels below the first leave out what .gitignore ignores.
- Read a file before editing or overwriting it.
- Use this, not `cat`, `head`, `tail`, `sed -n` or `ls` in the shell: it needs no permission, and an edit needs the file read here first.
