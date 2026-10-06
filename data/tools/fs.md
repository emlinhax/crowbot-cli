Move, copy or delete files and directories, or make a directory: what `mv`, `cp`, `rm` and `mkdir` do in the shell.

- `action` is `move`, `copy`, `delete` or `mkdir`. `path` is one path or a list of up to {fs_batch_max}.
- `move` and `copy` take `to`. When `to` is an existing directory, each path goes into it under its own name; otherwise the single path is moved or copied to `to`. A destination that already exists is refused: delete it first or pick another name.
- `copy` copies directories with everything in them. `mkdir` makes missing parent directories too.
- `delete` removes a file or an empty directory; a directory with contents needs `recursive: true`.
- Symbolic links, and anything holding the working directory, are refused.
- A moved file must be read at its new path before it is edited.
