# crowbot

A small terminal coding agent for [crowbot.sh](https://crowbot.sh). One backend, one binary.

Read `AGENTS.md` before changing anything; `NEXT_PLAN.md` tracks the phases.

## Commands

```sh
crowbot                          # interactive session
crowbot "fix the failing test"   # the same, with the prompt typed in for you to review
crowbot login                    # pair this machine (or: login --key <account number>)
crowbot signup                   # create an account: no email, no password, no recovery
crowbot -p "explain this repo"   # headless: one prompt, reply on stdout (pipe input works too)
crowbot --json "..."             # headless, one JSON event per line
crowbot models [--refresh]       # live models, prices and limits
crowbot help                     # every command
```

`--model <id>`, `--effort low|medium|high|max` and `--mode manual|auto|plan` apply to any run.
MANUAL asks before edits, commands and fetches; AUTO allows everything, deny rules included;
PLAN is read-only apart from its plan file and ends by handing the plan over. A headless run
declines every prompt, so use `--mode auto` there. The key comes from `CROWBOT_API_KEY`, else
`~/.crowbot/auth.json` (DPAPI-sealed on Windows, 0600 elsewhere).

## The session

The session takes over the whole terminal (the alternate screen) and gives it back on exit. The
conversation scrolls above; the message bar is pinned to the bottom, between two rules in the
mode's colour: the top one names the mode, the bottom one shows the model and effort, context
use and cost. While crowbot works, a raven flaps above the bar beside the time and tokens so far.
Select text with Shift+drag (the mouse otherwise belongs to crowbot); the session file under
`~/.crowbot/sessions/` keeps the whole conversation.

| Key | Does |
|---|---|
| Enter | send; while crowbot works, queue the message for when it is done |
| Tab | while crowbot works, steer (delivered after the current tool calls); otherwise complete a `/command` |
| `/` | at the start of the editor, list commands as you type: ↑↓ pick, Tab complete, Enter run, Esc hide |
| Shift+Enter, Ctrl+J, `\` then Enter | new line |
| Shift+Tab | cycle MANUAL (gray) → AUTO (purple) → PLAN (blue); switching to AUTO approves a waiting permission prompt |
| Esc | interrupt; queued messages come back to the editor |
| Ctrl+C | clear the editor, else interrupt, else quit on a second press |
| Ctrl+D | quit when the editor is empty |
| Ctrl+T | open every thinking block, or close them all; clicking one opens or closes just it |
| PgUp PgDn, mouse wheel | scroll the conversation; scrolling back to the end follows new output again |
| ↑ ↓ | history at the editor's edges |

Prompts replace the editor with a numbered card: permission (`1` yes, `2` no, `3` no and say
why, with a diff or command preview), a question from crowbot, or a finished plan. `/help`,
`/models`, `/mode [id]`, `/login`, `/logout` and `/quit` work inside the session. `/models` lists
the models to pick from (↑↓, then → or Enter) for the rest of the session; `/login` pairs this
device or takes an account number (shown masked); both take effect without a restart. Pasting works everywhere; on Windows,
where a paste arrives as keystrokes, a burst of keys is recognised as a paste so its Enters add
lines instead of sending.

## Layout

| Path | What lives there |
|---|---|
| `data/` | Every number, prompt, catalog, key and colour, embedded with `include_str!`. Change behaviour here first. |
| `data/limits.toml` | Every limit, each with its reason. |
| `data/endpoints.toml` | crowbot's origins and endpoints; `CROWBOT_API_URL` / `CROWBOT_CHAT_URL` override origins. |
| `data/commands/` | One spec per command (name, summary, usage, where it works). |
| `data/errors.toml` | Every error kind: title, hint, whether it is retried. |
| `data/prompts/` | System prompt pieces and mode reminders. |
| `data/modes/` | One file per permission mode (MANUAL, AUTO, PLAN): colour, verdicts, locked rules. |
| `data/tools/` | Each tool's description (`.md`) and argument schema (`.schema.json`). |
| `data/model_text.toml` | Everything crowbot tells the model on the user's behalf (declines, answers, errors). |
| `data/shells.toml` | Which shell runs commands, per OS, and its environment. |
| `data/theme.toml` | Colours by role, and syntax scopes mapped to roles. |
| `data/keybinds.toml`, `data/ui.toml` | Keys; the session's words, spinner, bottom-rule items, prompt choices, login card. |
| `data/tool_cards.toml` | How each tool call looks in the transcript. |
| `data/code_aliases.toml` | Code-fence languages mapped onto the bundled grammars. |
| `src/io/` | The only code that touches network, files, processes, terminal or clock (enforced by `clippy.toml`). |
| `src/api/` | crowbot endpoints, errors, SSE parsing, chat streaming, retry, pairing, signup. |
| `src/agent/` | The loop (`run.rs`), tool batches, prompts, context repair, doom-loop guard, shared run state. |
| `src/tools/` | One file per tool behind the `Tool` trait, registered in `tools/mod.rs`; `edit_match.rs` is the fuzzy matcher. |
| `src/permission/` | Rules (last match wins), the gate that layers them with the mode, shell splitting. |
| `src/tui/` | The session: `app.rs` loop, `layout.rs` + `screen.rs` (frame, row diff), `feed.rs` transcript blocks, `view.rs` scrolling, `status.rs` working line, editor, `frame.rs` rules, `boxed.rs` cards, `choice.rs` prompts, `palette.rs` popup, `picker.rs` models, `login.rs`. |
| `src/text/` | Styled lines, theme, markdown, tables, highlighting, diffs; shared by the TUI and the CLI. |
| `src/session/` | Append-only JSONL session files under `~/.crowbot/sessions/<project>/`. |
| `src/frontend/` | Headless `-p` / `--json` output. |
| `src/commands/` | One file per command, registered in `commands/mod.rs`. |
| `src/{app,cli,settings,paths,limits,mode}.rs` | Startup, routing, layered settings, locations, limits, modes. |
| `tests/e2e/` | The real binary against an in-process fake crowbot; `pty.rs` drives real sessions. |
| `tests/fixtures/` | Data the fake serves (`sse/*.sse`, `api/*.json`), sample `projects/`, `scenarios/*.toml`. |
| `tests/golden/edit/` | Edit-matcher cases: `before.txt` + `args.json` → `after.txt` or `error.txt`. |
| `tests/smoke.rs` | The only tests that touch the real API (`make smoke`). |
| `reference/` | Gitignored clones of opencode and pi plus crowbot's docs; designs are ported from here. |

Settings layer in this order, later wins: `data/defaults.toml`, `~/.crowbot/config.toml`,
`.crowbot/config.toml` in the project, then flags; `[[permission]]` rules append instead. That is
also where permanent allowances go, e.g. `permission = "bash"`, `pattern = "cargo *"`,
`action = "allow"`. `CROWBOT_HOME` moves `~/.crowbot`.

## Checks

GNU make, run from Git Bash on Windows (`winget install ezwinports.make`).

| Target | Runs | When |
|---|---|---|
| `make lint` | `cargo fmt --check`, `clippy -D warnings` (includes the io-wrapper rule) | before every push |
| `make test` | unit, snapshot, renderer and data-integrity tests | before every push |
| `make e2e` | binary vs fake crowbot, including PTY sessions | CI on every push to `main`; locally only for critical changes |
| `make golden` | rewrite failing goldens and snapshots | only for a deliberate change |
| `make smoke` | real API round trip, needs `CROWBOT_SMOKE_KEY` (costs a fraction of a cent) | by hand, never in CI |
| `make build` | static release binary | as needed |

## Conventions

- Comments say why, briefly. `CEILING:` marks a known shortcut and its upgrade path: `grep -rn CEILING: src`.
- Commit straight to `main`; `prod` is the release trigger and is merged into only on an explicit go.
