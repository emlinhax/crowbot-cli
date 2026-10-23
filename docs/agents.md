# Subagents and agent files

How the `task` tool's subagents are put together: where kinds come from, what each one is told,
which tools and model it gets, and how several work at once without colliding. The README says how
to use them; this says why they are the way they are. The examples in `docs/agents/` are agent
files to copy.

## What a subagent is

The model hands a self-contained job to a subagent with `task`: another crowbot run with its own
conversation, which works through the job and replies once. Its reply is the task call's result and
nobody else sees its steps. That is the point of it: a broad search or a contained change spends the
subagent's context, not the session's.

It runs under the session's mode and permission rules and asks through the session's cards. Its
edits count for the prompt's `/undo`, its cost is the session's, and its conversation is saved next
to the session's file. Limits are in `data/limits.toml` (`subagent.*`): one level deep (a subagent
starts no subagents of its own), two at a time (the plan answers two requests at once), and 30
requests each.

## Where kinds come from

A kind is a name, a description, a tool list, a prompt, and optionally a model, effort and `when`.

| Source | Where | Trust |
|---|---|---|
| Built in | `data/agents/*.toml`, registered in `src/agent/kinds.rs` | shipped |
| Yours | `~/.crowbot/agents/*.md` | always read |
| The project's | `.crowbot/agents/*.md` | read once the folder is trusted |

They are read in that order and a later kind of the same name replaces an earlier one, so an
`explore.md` of your own replaces the built-in `explore`. A project's agent files are listed in the
trust prompt beside its settings, because a prompt steers what its subagents do just as a setting
does. Until the folder is trusted, they are left out (a headless run says so on stderr).

Kinds are read once, at start, before the settings: a changed file counts from the next session.

## Agent files

Claude Code's shape, so an agent written for one works in the other with at most a tool name to
adjust:

```markdown
---
name: reviewer
description: Reviews a change for bugs and reports each finding with file and line.
when: after a change is made and before calling it done
tools: Read, Grep, Glob, Bash
effort: high
---
You are a subagent: a code reviewer handed one change to review…
```

| Key | Meaning |
|---|---|
| `name` | The kind's id: letters, digits, `-`, `_`. Required. |
| `description` | What it does, as the model choosing a kind reads it. Required. |
| `when` | The jobs to pick it for, shown after the description (`Pick it for: …`). |
| `tools` | The session's tools it gets, by name; left out, all of them. Claude Code's names (`Read`, `MultiEdit`) are mapped (`data/agent_files.toml`); an MCP tool is `server__tool`; an unknown name is dropped with a note. |
| `except` | Tools to leave out of the full set. |
| `model` | The model it runs on unless settings or the call name another; `inherit` is none. |
| `effort` | Reasoning effort, one of `data/effort.toml`'s levels. |
| `base` | `append` (default) or `replace`; see below. |
| `color` | Claude Code's; read and ignored. |

The header is `key: value` lines, not YAML: a list is `a, b` or `[a, b]`, quotes around a value are
dropped, and a value cannot span lines. A file that does not parse is skipped with a note naming the
file and the problem; it never stops crowbot starting.

## What a subagent is told

**System prompt.** With `base: append`, the default, a subagent gets the session's system prompt,
with the environment block's `Model:` line set to the model the subagent runs on, then its kind's
prompt. That way it keeps the project's `AGENTS.md`/`CLAUDE.md`, the tool guidance and the MCP and
forum notes the session has. It also means every subagent of one kind on one model sends the same
prefix, which vendors that cache prompt prefixes can reuse. With `base: replace` it gets only its
kind's prompt and the environment block (directory, platform, date, model). That suits a narrow
persona that the session's guidance would only distract, but it drops the project's instructions,
so the kind's prompt has to carry whatever it needs.

**First message.** The job, as the `task` call wrote it in `prompt`. A kind that can change files
also gets, ahead of the job, who else is at work and the rules for scopes (below). A read-only kind
is spared that text: it cannot collide with anyone.

**What a good kind prompt says**, as the built-in ones and the examples do:

- That it is a subagent: nobody sees its steps and only its last reply goes back. Models otherwise
  narrate to a user who is not there, or stop to ask questions nobody will answer.
- What it may and may not do, said once and plainly ("you change nothing").
- How to work, as an order of steps the job needs (reproduce, narrow, fix, prove), not a list of
  virtues.
- What to reply with, concretely: paths with line numbers, the commands run and what they showed,
  and what was not verified. The caller has to act on the reply without seeing the work.

## Tools

A kind's tools are a subset of the session's: the names in `tools` (or all of them for `*` or no
list), minus `except`. Two are adjusted on top of that:

- `task` is never given at the depth limit, so a subagent is never offered a call it could not make.
- `claim` is added for any kind that has `edit`, `write` or `fs`, unless its `except` names it. A
  subagent that can change files needs it to reach past its scope. The session's own run is never
  offered `claim`, since only subagents hold files.

Tools are where a kind's limits are enforced; the prompt only explains them. A read-only kind is
read-only because it has no tool that changes anything, not because its prompt says so. The mode
still applies on top: a kind cannot do more than the session's mode allows, and (a known ceiling)
cannot be made stricter than it either.

A tool list that differs from the session's means the vendor cannot reuse the session's cached
prefix for it (noted as a ceiling in `src/agent/child.rs`).

## Model and effort

Each subagent's model is settled once, as it starts (`src/agent/roles.rs`), first match wins:

1. a `model` the `task` call names (allowed only if `subagents.choices`, when set, lists it);
2. your setting for the kind: `/models`, or `[subagents.kinds.<name>]` in `config.toml`;
3. the kind's own `model`, from its agent file;
4. your setting for every kind: `[subagents]`;
5. the chat's model.

Effort is settled the same way on its own, and dropped for a model that does not reason. A model
crowbot does not serve fails the call with where it came from; nothing falls back to another model
silently. Costs are priced per request with the model it went to, so a cheap kind is billed cheaply.

The system prompt lists each kind's model and the models a call may name, with their prices and
windows, so the model can pick a stronger or cheaper one for one job. In testing it also used this to
retry on another model after a vendor error.

## Working at the same time: scopes and claims

Subagents of one batch start together, so two `general` subagents could otherwise edit one file
blind. `src/agent/claims.rs` keeps one table per prompt:

- A `task` call may give a `scope`: the paths its subagent may change (a directory covers what is
  under it; `*` matches anything). A scope that overlaps a running subagent's does not start.
- At the permission gate, `edit`, `write` and `fs` are refused outside a scoped subagent's scope, and
  on any path a running subagent holds. That includes the session's own run until the holder ends.
- `claim` widens a subagent's scope when nobody holds the path. Otherwise it says who does and what
  they are doing. With `wait: true` it waits for them to finish, unless they are themselves waiting
  on the caller, which would never end.
- A subagent with no scope may change anything nobody holds.

The table answers for the agent that started the subagents. That agent is blocked waiting on their
results and cannot be asked mid-batch.

Known ceilings, each marked `CEILING:` in `claims.rs`: the shell is not checked (a `bash` command
that writes files is not seen; the roster tells subagents to change files only through the tools),
overlap is judged from the part of a pattern before its first wildcard and so errs towards refusing,
and paths compare case-sensitively. The upgrade for the first is a git worktree per scoped subagent.

## The examples

`docs/agents/` holds six kinds that cover what `explore` and `general` do not. Copy any of them to
`~/.crowbot/agents/` and adjust. Their effort levels are a starting point: all but `reverser` were
run on a cheap model at these settings, each picked from its description without being named.

| Kind | Tools | Effort | Why this shape |
|---|---|---|---|
| `reviewer` | Read, Grep, Glob, Bash | high | Bash for `git diff` and to run a check that settles a finding; no edit tools, so a review cannot turn into a rewrite. Reports only what it verified, most severe first. |
| `debugger` | everything but `plan_exit` | high | It must reproduce, change and re-run, so it needs the full set. Its prompt orders the work (reproduce, narrow, name the cause, fix, prove) and forbids skipping or loosening tests. |
| `tester` | Bash, Read, Grep, Glob | low | Running a suite is mechanical, so it gets low effort and no edit tools. It finds the project's own test command, re-runs a flaky test once, and trims errors to what explains them. |
| `planner` | Read, Grep, Glob, Bash, WebFetch | high | A plan needs the code as it is: callers, conventions, what to reuse. It edits nothing, and its reply is a fixed outline (goal, files, steps, risks, verification). |
| `researcher` | WebFetch, Read, Grep, Glob, Bash | medium | Answers from primary sources for the version the project pins. It treats forum answers as leads and web content as data, and settles what it can by running code. |
| `reverser` | Decompile, Bash, Read, Grep, Glob, Write, WebFetch | high | crowbot's decompiler, scripts to recover and check logic, `write` for scratch scripts, and the web for formats and protocols. It may not patch the binary it was given. |

Every example states it is a subagent and what its reply must hold, and names what it must not do
both in its prompt and in its tool list.
