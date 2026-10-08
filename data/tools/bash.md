Run a shell command in the working directory. {shell_note}

- Use it for builds, tests, git and other programs, not for what a tool does: `read` rather than `cat`, `head`, `tail`, `sed -n` or `ls`; `glob` rather than `find`; `grep` rather than `grep` or `rg`; `fs` rather than `mv`, `cp`, `rm` or `mkdir`.
- Commands cannot read input; anything interactive fails. Pass flags that avoid prompts.
- Output is stdout and stderr together. Long output keeps its end and saves the full text to a file whose path is given; don't pipe it into `tail`.
- `timeout` is in seconds (default {bash_timeout_secs}, max {bash_max_timeout_secs}). Processes still running when the command finishes are stopped.
