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
crowbot --json "..."             # headless, one JSON event per line (fields stable; additions only)
crowbot models [--refresh]       # live models, prices and limits
crowbot help                     # every command
```

`--model <id>`, `--effort <level>` and `--mode <mode>` go before the prompt or command;
`crowbot --help` lists their values. A first word that names a command runs it, except with `-p`
or `--json`, where every word is prompt text. Otherwise the session opens when stdin and stdout
are both terminals, and the prompt runs headless when they are not. Piped stdin is read to its
end and appended to the words (`< /dev/null` when stdin is an open pipe with nothing coming). Exit codes: `0` done, `1` error, rejection or turn limit, `2` bad usage, `130`
interrupted.

MANUAL asks before edits, commands and fetches; AUTO allows everything, deny rules included.
PLAN locks out edits outside its plan file and runs, unasked, only the commands its allowlist
names (`data/modes/plan.toml`); the allowlist is not a sandbox. It ends by handing the plan over.
A headless run declines every prompt: allow what it needs with `[[permission]]` rules, or use
`--mode auto`. The key comes from `CROWBOT_API_KEY`, else
`~/.crowbot/auth.json` (DPAPI-sealed on Windows, 0600 elsewhere).

## The session

The session takes over the whole terminal (the alternate screen) and gives it back on exit. The
conversation scrolls above; the message bar is pinned to the bottom, between two rules in the
mode's colour, and the bottom one carries the status on its right: the mode, the model and effort,
context use and cost. While crowbot works, a raven flaps above the bar beside the time and tokens so far.
Right-click copies a message whole (yours, a reply, a tool's output, the pairing code), with a
"Copied" note at the top right; Shift+drag selects any text, since the mouse otherwise belongs to
crowbot. The session file under `~/.crowbot/sessions/` keeps the whole conversation.

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
why, with a diff or command preview), a question from crowbot, or a finished plan. Inside the
session, `/help` lists the commands:

| Command | Does |
|---|---|
| `/new` (or `/clear`) | a fresh conversation in a new session file; the last one stays saved |
| `/models`, `/effort [level]`, `/mode [id]` | model, reasoning effort or permission mode for the rest of the session |
| `/status` | account and balance, model, mode, context use, cost so far, session file |
| `/copy` | the last reply to the clipboard, as markdown |
| `/init` | crowbot studies the project and writes its `AGENTS.md` |
| `/login`, `/logout` | pair this device or take an account number (shown masked), or forget the key |
| `/quit` (or `/exit`) | leave |

`/models` lists the models to pick from (↑↓, then → or Enter); `/login` takes effect without a
restart. The project's `AGENTS.md` (or else `CLAUDE.md`) joins the system prompt when a
conversation starts, up to `agent.instructions_bytes`. Pasting works everywhere; on Windows,
where a paste arrives as keystrokes, a burst of keys is recognised as a paste so its Enters add
lines instead of sending.

## Layout

| Path | What lives there |
|---|---|
| `data/` | Every number, prompt, catalog, key and colour, embedded with `include_str!`. Change behaviour here first. |
| `data/limits.toml` | Every limit, each with its reason. |
| `data/endpoints.toml` | crowbot's origins and endpoints; `CROWBOT_API_URL` / `CROWBOT_CHAT_URL` override origins. |
| `data/commands/` | One spec per command (name, summary, usage, where it works). |
| `data/errors.toml` | Every error kind: title, hint, and whether it is retried (else its status decides). |
| `data/prompts/` | System prompt pieces and mode reminders. |
| `data/modes/` | One file per permission mode (MANUAL, AUTO, PLAN): colour, verdicts, locked rules. |
| `data/tools/` | Each tool's description (`.md`) and argument schema (`.schema.json`). |
| `data/model_text.toml` | Everything crowbot tells the model on the user's behalf (declines, answers, errors). |
| `data/shells.toml` | Which shell runs commands, per OS, and its environment. |
| `data/theme.toml` | Colours by role, and syntax scopes mapped to roles. |
| `data/keybinds.toml`, `data/ui.toml` | Keys; the session's words, spinner, bottom-rule items, prompt choices, login card. |
| `data/tool_cards.toml` | How each tool call looks in the transcript. |
| `data/code_aliases.toml` | Code-fence languages mapped onto the bundled grammars. |
| `src/io/` | The only code that touches network, files, processes, terminal or clock (enforced by `clippy.toml`). `http.rs` is crowbot's API (reqwest); `fetch.rs` is the rest of the web, for webfetch (cffetch). |
| `crates/cffetch/` | The user's Cloudflare-aware HTTP client, vendored unchanged (why it is outside the workspace: `Cargo.toml`). Re-sync by copying `src/`, `tests/`, `examples/` and `Cargo.toml` over; its own suite runs from that directory (it hits the network). |
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
`.crowbot/config.toml` in the start directory, then flags; `[[permission]]` rules append instead.
Permanent allowances go in `~/.crowbot/config.toml`, e.g. `permission = "bash"`,
`pattern = "cargo *"`, `action = "allow"`. A folder's own `.crowbot/config.toml` may set `model`,
`effort` and deny rules; anything else (mode, shell, allow or ask rules) needs the folder trusted.
A session started there shows what the file asks for and asks first, and remembers a yes in
`~/.crowbot/trusted.json`; a headless run ignores those settings and says so on stderr.
`CROWBOT_HOME` moves `~/.crowbot`.

## Checks

GNU make, run from Git Bash on Windows (`winget install ezwinports.make`).

webfetch fetches through cffetch, whose BoringSSL is built from source, so a build also needs
CMake and libclang (`LIBCLANG_PATH` pointing at LLVM's `bin` when it is not found on its own),
plus NASM on Windows (`winget install Kitware.CMake LLVM.LLVM NASM.NASM`). CI installs them
through `.github/actions/setup`. The binary carries two TLS stacks: rustls for crowbot's API,
BoringSSL for the web.

| Target | Runs | When |
|---|---|---|
| `make lint` | `cargo fmt --check`, `clippy -D warnings` (includes the io-wrapper rule) | before every push |
| `make test` | unit, snapshot, renderer and data-integrity tests | before every push |
| `make e2e` | binary vs fake crowbot, including PTY sessions | CI on every push to `main`; locally only for critical changes |
| `make golden` | rewrite failing goldens and snapshots | only for a deliberate change |
| `make smoke` | real API round trip, needs `CROWBOT_SMOKE_KEY` (costs a fraction of a cent) | by hand, never in CI |
| `make build` | static release binary | as needed |

## Releases

Pushing `prod` (on an explicit go only) runs `.github/workflows/release.yml`: the full CI checks,
then a GitHub release tagged `v<version>-<run>` holding one binary per target and `SHA256SUMS`.
Asset names never change, so these always serve the newest release:

| Platform | Download |
|---|---|
| Linux x86_64 (static) | `https://github.com/emlinhax/crowbot-cli/releases/latest/download/crowbot-x86_64-unknown-linux-musl` |
| macOS Apple silicon | `https://github.com/emlinhax/crowbot-cli/releases/latest/download/crowbot-aarch64-apple-darwin` |
| Windows x86_64 | `https://github.com/emlinhax/crowbot-cli/releases/latest/download/crowbot-x86_64-pc-windows-msvc.exe` |

A target is one entry in `ci.yml`'s matrix. The binaries are unsigned, so macOS Gatekeeper and
Windows SmartScreen warn about a copy downloaded by a browser; `curl` downloads are not flagged.
