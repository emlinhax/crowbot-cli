Hands one self-contained job to a subagent: another crowbot, with tools of its own, that works through it and replies once. Its reply is this call's result; the user does not see it, so tell them what matters from it. Use it for work whose steps you do not need to watch, such as a broad search across the project, to keep your own context for the task in hand. The subagent knows nothing of this conversation: put everything it needs in `prompt`, and say what its reply should hold. Calls in the same reply run at the same time.

Kinds of subagent (`subagent_type`):
{agents}
