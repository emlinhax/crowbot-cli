Run a shell command in the working directory. {shell_note}

- Use it for builds, tests, git and other programs. Prefer `read`, `glob` and `grep` over `cat`, `find` and `grep` in the shell.
- Commands cannot read input; anything interactive fails. Pass flags that avoid prompts.
- Output is stdout and stderr together; long output keeps the end and the full text is saved to a file whose path is given.
- `timeout` is in seconds (default {timeout}, max {max_timeout}). Processes still running when the command finishes are stopped.
