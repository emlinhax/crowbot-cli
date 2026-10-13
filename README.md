# crowbot

A small terminal coding agent for [crowbot.sh](https://crowbot.sh). One backend, one binary.

Read `AGENTS.md` before changing anything; `NEXT_PLAN.md` tracks the phases.

## Install

| Platform | Do |
|---|---|
| Windows | Download [crowbot-x86_64-pc-windows-msvc.exe](https://github.com/emlinhax/crowbot-cli/releases/latest/download/crowbot-x86_64-pc-windows-msvc.exe) and double-click it; it offers to install itself for you and puts `crowbot` on your PATH. |
| macOS (Apple silicon) | `curl -fsSL -o crowbot https://github.com/emlinhax/crowbot-cli/releases/latest/download/crowbot-aarch64-apple-darwin && chmod +x crowbot && ./crowbot install` |
| Linux (x86_64) | `curl -fsSL -o crowbot https://github.com/emlinhax/crowbot-cli/releases/latest/download/crowbot-x86_64-unknown-linux-musl && chmod +x crowbot && ./crowbot install` |

`crowbot install` copies the program to `%LOCALAPPDATA%\Programs\crowbot` (Windows) or
`~/.local/bin` and puts it on PATH: on Windows directly, with an Apps & Features entry; elsewhere
it prints the line for your shell profile. `crowbot uninstall` undoes it and keeps `~/.crowbot`.
Binaries are unsigned: Windows asks for "More info → Run anyway"; macOS blocks a browser download
until Privacy & Security → Open Anyway (`curl` downloads are not flagged).

Sessions check for a new release daily, verify it against `SHA256SUMS` and swap it in; the next
start says so. `crowbot update` does it now. `auto_update = false` or `CROWBOT_NO_UPDATE=1` turns
it off; development builds never update.

## Commands

```sh
crowbot                          # interactive session
crowbot "fix the failing test"   # the same, with the prompt typed in for you to review
crowbot login                    # pair this machine (or: login --key <account number>)
crowbot logout                   # forget this machine's key
crowbot signup                   # create an account: no email, no password, no recovery
crowbot -p "explain this repo"   # headless: one prompt, reply on stdout (pipe input works too)
crowbot --json "..."             # headless, one JSON event per line (fields stable; additions only)
crowbot models [--refresh]       # live models, prices and limits
crowbot forums                   # forums the agent can browse (add, log in, list)
crowbot update                   # the newest release, now
crowbot install / uninstall      # put crowbot on your PATH, or take it off
crowbot help                     # every command
```

## Forums

crowbot can browse web forums that run the Tapatalk plugin, so the agent can read threads and
search them for you. UnknownCheats is enabled out of the box; add more and log in with
`crowbot forums` (or `/forums` in a session):

```sh
crowbot forums                      # list the forums the agent can browse
crowbot forums add <url or name>    # find a forum through Tapatalk's directory and add it
crowbot forums login <forum>        # log in with your own account (password is never stored)
crowbot forums logout <forum>       # drop the session, stay added as a guest
crowbot forums remove <forum>       # forget it
```

In a session, `/forums` opens the list as a card: pick a forum to log in (username, then a hidden
password), log out or remove it, or add one from Tapatalk's directory through the last row.
`/forums login <forum>` opens it straight at that forum's login. Only the resulting session cookie
is kept, sealed in `~/.crowbot/forums.json` (DPAPI on Windows, 0600 elsewhere) — never the password.

The system prompt names each added forum and its hint (seeded ones from `data/forums.toml`;
wording in `data/prompts/forums.toml`), so the agent checks there first. Read tools
(`forum_sections`, `forum_topics`, `forum_thread`, `forum_search`) run unasked; `forum_post` shows
the exact text and asks every time, even in AUTO, and never runs headless. Guests can read; most
forums need a login to search. Forum content is untrusted data, never instructions. Deny with a
rule, e.g. `permission = "forum_post", pattern = "*", action = "deny"`.

Flags (`--model`, `--effort`, `--mode`; values in `crowbot --help`) go before the prompt or
command. A first word naming a command runs it, except after `-p`/`--json`, where every word is
prompt. Otherwise a session opens when stdin and stdout are terminals, else the prompt runs
headless. Piped stdin is read to its end and appended to the words (`< /dev/null` if the pipe
never closes). Exit codes: `0` done, `1` error, rejection or turn limit, `2` bad usage, `130`
interrupted.

MANUAL asks before edits, commands and fetches; AUTO allows everything, deny rules included.
PLAN edits only its plan file, fetches freely, and runs unasked only the commands in
`data/modes/plan.toml` (git reads, listings, builds, tests, linters; these run project code, so it
is not a sandbox) plus trailing pipe filters (`data/filters.toml`). It ends by handing over the
plan.
A headless run declines every prompt: allow what it needs with `[[permission]]` rules, or use
`--mode auto`. The key comes from `CROWBOT_API_KEY`, else
`~/.crowbot/auth.json` (DPAPI-sealed on Windows, 0600 elsewhere).

## The session

The session takes over the whole terminal (the alternate screen) and gives it back on exit. The
conversation scrolls above; the message bar is pinned to the bottom, between two rules in the
mode's colour, and the bottom one carries the status on its right: the mode, the model and effort,
context use and cost. While crowbot works, a murmuration of crows ripples above the bar beside the
time and tokens so far. Right-click copies a message whole (yours, a reply, a tool's output, the
pairing code), with a "Copied" note at the top right; Shift+drag selects any text, since the mouse
otherwise belongs to crowbot. The session file under `~/.crowbot/sessions/` keeps the whole
conversation.

| Key | Does |
|---|---|
| Enter | send; while crowbot works, queue the message for when it is done |
| Tab | while crowbot works, steer (delivered after the current tool calls); otherwise complete a `/command` |
| `/` | at the start of the editor, list commands as you type: ↑↓ pick, Tab complete, Enter run, Esc hide |
| Shift+Enter, Ctrl+J, `\` then Enter | new line |
| Shift+Tab | cycle MANUAL (gray) → PLAN (blue) → AUTO (purple); switching to AUTO approves a waiting permission prompt |
| Esc | interrupt; queued messages come back to the editor |
| Ctrl+C | clear the editor, else interrupt, else quit on a second press |
| Ctrl+D | quit when the editor is empty |
| Ctrl+T | open every thinking block, or close them all; clicking one opens or closes just it |
| PgUp PgDn, mouse wheel | scroll the conversation; scrolling back to the end follows new output again |
| ↑ ↓ | history at the editor's edges |

Prompts replace the editor with a numbered card: permission (`1` yes, `2` no, `3` no and say
why, with a diff or command preview), crowbot's questions (up to four on one card, a tab each,
some taking several answers, all reviewed before they go back), or a finished plan. Inside the
session, `/help` lists the commands:

| Command | Does |
|---|---|
| `/new` (or `/clear`) | a fresh conversation in a new session file; the last one stays saved |
| `/models`, `/effort [level]`, `/mode [id]` | model, reasoning effort or permission mode for the rest of the session |
| `/status` | account and balance, model, mode, context use, cost so far, session file |
| `/copy` | the last reply to the clipboard, as markdown |
| `/init` | crowbot studies the project and writes its `AGENTS.md` |
| `/login`, `/logout` | pair this device or take an account number (shown masked), or forget the key |
| `/forums` | the forums the agent can browse: log in, log out, remove, add |
| `/quit` (or `/exit`) | leave |

`/login` takes effect without a restart. The project's `AGENTS.md` (else `CLAUDE.md`) joins the
system prompt at conversation start, up to `agent.instructions_bytes`. On Windows a paste arrives
as keystrokes; a key burst counts as a paste, so its Enters add lines.

## Environment

| Variable | Does |
|---|---|
| `CROWBOT_HOME` | moves `~/.crowbot` (keys, sessions, settings, caches) |
| `CROWBOT_API_KEY` | the key to use, over any stored one |
| `CROWBOT_NO_UPDATE` | turns the daily self-update off, as `auto_update = false` does |
| `CROWBOT_API_URL`, `CROWBOT_CHAT_URL` | point crowbot's API and its chat site elsewhere (`data/endpoints.toml`) |
| `CROWBOT_RELEASES_URL` | where the updater lists releases (GitHub's API by default) |

## Layout

| Path | What lives there |
|---|---|
| `data/` | Every number, prompt, catalog, key, word and colour, embedded with `include_str!`; one file (or folder) per concern, each opening with what it holds. Change behaviour here first. |
| `data/limits.toml` | Every limit, each with its reason. |
| `data/endpoints.toml` | crowbot's origins and endpoints, and the variables that override them. |
| `src/io/` | The only code that touches the network, files, processes, terminal or clock; `clippy.toml` bans the raw calls elsewhere. crowbot's API goes through reqwest, the rest of the web (webfetch, forums) through cffetch. |
| `src/api/` | crowbot's endpoints, errors, streaming chat, retries, pairing and signup. |
| `src/agent/` | The run loop, tool batches, what the model is sent, and the session setup both frontends share. |
| `src/live/` | What a run shares with its tools and frontend while it is going: events, prompts, state, files read. |
| `src/conversation/` | Messages, parts and errors as a conversation holds them. |
| `src/tools/` | One file or folder per tool behind the `Tool` trait, registered in `tools/mod.rs`. |
| `src/permission/` | Rules (last match wins), the gate that layers them with the mode, the bash command split. |
| `src/forums/` | Forums: the directory, the sealed store, accounts and the Tapatalk backend. |
| `src/models/` | The model catalog (live, cached or bundled) and its table. |
| `src/commands/` | One file per command (effect-only ones share `fixed.rs`), registered in `commands/mod.rs`. |
| `src/frontend/` | What drives a run: `print` (headless) and `tui/` (the session), and the words and tool cards both share. |
| `src/text/` | Styled lines, theme, markdown, tables, highlighting, diffs. |
| `src/session/` | Append-only JSONL session files under `~/.crowbot/sessions/<project>/`, and their reader. |
| `src/battery/` | The hidden `crowbot battery report`: a battery's runs counted by tool, shell command, stand-in, sequence and replayed gate verdict, then candidate changes ranked by count. |
| `src/config/` | Layered settings, locations, limits and the trusted-folder list. |
| `src/release/` | Release identity, install and self-update. |
| `src/*.rs` | Startup (`app`), routing (`cli`), the key (`auth`), data loading, efforts and modes. |
| `crates/cffetch/` | The user's Cloudflare-aware HTTP client, vendored unchanged (outside the workspace: see `Cargo.toml`). Re-sync: copy `src/`, `tests/`, `examples/`, `Cargo.toml`. Its networked suite runs from that directory. |
| `tests/e2e/` | The real binary against an in-process fake crowbot; `pty/` drives real sessions. |
| `tests/fixtures/` | What the fake serves (`sse/`, `api/`), sample `projects/`, `scenarios/*.toml`, frozen `session/` files. |
| `tests/golden/` | Edit-matcher cases (`edit/`) and the `--json` stream (`json/`). |
| `tests/smoke.rs` | The only tests that touch the real API (`make smoke`). |
| `tests/battery/` | The tool-use battery's runner: each task headless in a fresh copy of its repo, then checked and recorded; its own tests drive it against the fake. |
| `bench/` | The battery's corpus: pinned repos, tasks, planted bugs, hidden checks, and the baseline run to compare a re-run with. `cache/` and `runs/` are gitignored. |
| `reference/` | Clones of opencode and pi plus crowbot's docs; local only, see AGENTS.md. |

Settings layer, later wins: `data/defaults.toml`, `~/.crowbot/config.toml`,
`./.crowbot/config.toml`, flags; `[[permission]]` rules append. A project config may set `model`,
`effort` and deny rules; anything else needs the folder trusted (a session shows what it asks for
and remembers a yes in `~/.crowbot/trusted.json`; a headless run ignores those settings, with a
note on stderr). Permanent allowances go in `~/.crowbot/config.toml`, e.g. `permission = "bash"`,
`pattern = "cargo *"`, `action = "allow"`.

## Checks

GNU make (see the Makefile's first line).

webfetch goes through cffetch, which builds BoringSSL from source: a build needs CMake and
libclang (`LIBCLANG_PATH` = LLVM's `bin` if not found), plus NASM on Windows
(`winget install Kitware.CMake LLVM.LLVM NASM.NASM`); CI installs them in `.github/actions/setup`.
TLS: rustls for crowbot's API, BoringSSL for the web.

| Target | Runs |
|---|---|
| `make lint` | `cargo fmt --check`, `clippy -D warnings` (includes the io-wrapper rule) |
| `make test` | unit, snapshot, renderer and data-integrity tests |
| `make e2e` | binary vs fake crowbot, including PTY sessions and the battery's runner |
| `make golden` | rewrite failing goldens and snapshots |
| `make smoke` | real API round trip, needs `CROWBOT_SMOKE_KEY` (costs a fraction of a cent) |
| `make battery-setup` | clone the battery's repos into `bench/cache/`, install their dependencies, check every planted bug fails its check (runs the repos' code) |
| `make battery` | every task against the real model into `bench/runs/<stamp>/`; needs `CROWBOT_SMOKE_KEY`, stops at `cap_micros`; `TASKS=a,b REPEATS=1` runs a pilot |
| `make build` | static release binary |

When to run each: AGENTS.md.

## Releases

Pushing `prod` runs `.github/workflows/release.yml`: the CI checks, then a release tagged
`v<version>-<run>` with one binary per target and `SHA256SUMS`, built with that tag
(`CROWBOT_RELEASE`) for `--version` and the updater. Asset names are fixed, so
`releases/latest/download/<asset>` (the Install links) always serves the newest. Adding a target:
see `data/release.toml`'s header.

## Licence

crowbot is licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at
your option. Unless you say otherwise, a contribution you submit is licensed the same way.
