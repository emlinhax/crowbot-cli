Run a shell command in the working directory. {shell_note}

- Use it for builds, tests, git and other programs, not for what a tool does: `read` rather than `cat`, `head`, `tail`, `sed -n` or `ls`; `glob` rather than `find`; `grep` rather than `grep` or `rg`; `fs` rather than `mv`, `cp`, `rm` or `mkdir`.
- Long output already keeps its end and saves the rest to a file, so there is no need to pipe it into `tail`.
- Commands cannot read input; anything interactive fails. Pass flags that avoid prompts.
- Output is stdout and stderr together; long output keeps the end and the full text is saved to a file whose path is given.
- `timeout` is in seconds (default {timeout}, max {max_timeout}). Processes still running when the command finishes are stopped.
