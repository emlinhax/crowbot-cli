Replace exact text in a file. Read the file first.

- Each edit replaces `old_text` with `new_text`. `old_text` must match the file exactly once (include enough surrounding lines to make it unique), unless `replace_all` is set.
- All edits in one call are matched against the file as it was before the call, and must not overlap.
- Copy `old_text` from the file content, without the line-number prefix `read` shows.
- Small whitespace or quote differences are tolerated, but exact text is always safest.
