# crowbot

A small terminal coding agent for [crowbot.sh](https://crowbot.sh). One backend, one binary.

Read `AGENTS.md` before changing anything; `NEXT_PLAN.md` tracks the phases.

## Install

| Platform | Do |
|---|---|
| macOS (Apple silicon), Linux (x86_64) | `curl -fsSL https://github.com/emlinhax/crowbot-cli/releases/latest/download/install.sh \| sh` |
| Windows (x64, PowerShell) | `irm https://github.com/emlinhax/crowbot-cli/releases/latest/download/install.ps1 \| iex` |

The script (`install/`) picks the release file for the machine, checks it against `SHA256SUMS`
and runs `crowbot install`. By hand: download `crowbot-<target>` from the
[latest release](https://github.com/emlinhax/crowbot-cli/releases/latest), then
`chmod +x` it and run `./crowbot install`; on Windows, double-click the `.exe` and it offers to
install itself. `crowbot install` copies the program to `%LOCALAPPDATA%\Programs\crowbot`
(Windows) or `~/.local/bin` and puts it on PATH: on Windows directly, with an Apps & Features
entry; elsewhere it prints the line for your shell profile. `crowbot uninstall` undoes it and
keeps `~/.crowbot`. Binaries carry no Apple or Windows code signature: a browser download asks
for "More info → Run anyway" on Windows and Privacy & Security → Open Anyway on macOS, while the
scripts' downloads are not flagged.

Sessions check for a new release daily and swap it in once its `SHA256SUMS` carries crowbot's
minisign signature for that release and the file matches it; the next start says so.
`crowbot update` does it now. `auto_update = false` or `CROWBOT_NO_UPDATE=1` turns it off;
development builds never update.

## Commands

```sh
crowbot                          # interactive session
crowbot "fix the failing test"   # the same, with the prompt typed in for you to review
crowbot --continue               # go on with this project's latest session (with -p too)
crowbot login                    # pair this machine (or: login --key <account number>)
crowbot logout                   # forget this machine's key
crowbot signup                   # create an account: no email, no password, no recovery
crowbot -p "explain this repo"   # headless: one prompt, reply on stdout (pipe input works too)
crowbot --json "..."             # headless, one JSON event per line (fields stable; additions only)
crowbot models [--refresh]       # live models, prices and limits
crowbot presets [new <name>]     # presets and what each picks; `new` writes one to edit
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
context use, cost, the plan's allowance left and the balance (orange when the account cannot
spend). The account is asked at the start, on a new key, at the end of each run and at most
every `tui.account_refresh_secs` mid-run; an idle session never asks. While crowbot works, a murmuration of crows ripples above the bar beside the
time and tokens so far. Right-click copies a message whole (yours, a reply, a tool's output, the
pairing code), with a "Copied" note at the top right; Shift+drag selects any text, since the mouse
otherwise belongs to crowbot. The session file under `~/.crowbot/sessions/` keeps the whole
conversation, branches taken back included; `--continue` and `/resume` go on with one, with the
model, effort and mode it last used (a headless run keeps the mode it is given).

In a git work tree, crowbot snapshots the files before a prompt's first change into a shadow git
under `~/.crowbot/snapshots/` (the project's `.gitignore` applies; its own `.git` is never
touched), so `/undo` and `/retry` put back what that prompt changed; a file edited since is left
alone. Outside one, they take back the conversation only. When a request nears the model's window
(`compaction.threshold_pct`), or crowbot refuses one as too long, earlier turns are summarised by
the model and sent ahead of the last ones (`compaction.keep_tokens`), word for word; `/compact`
does it by hand.

The model can hand a self-contained job to a subagent with the `task` tool: `explore` reads and
searches and changes nothing, `general` has the same tools as the session. Kinds are files in
`data/agents/`. A subagent runs under the session's mode and asks through its permission cards; a
plain No to one of its calls stops the whole run. It runs on the session's model, or on what the
session's preset picks for its kind (Presets, below). Its reply is the tool's result (its card shows
its latest call while it works), its cost is the session's, and its edits are the prompt's for
`/undo`. Its conversation is saved beside the session's, in a folder named for it, so `/resume`
lists sessions only. `--json` wraps a subagent's events in `{"type":"child","call_id":…}`; add up
cost from `message_end` events at any depth, or from the task's `tool_end` usage, not both.

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
| `/models`, `/effort [level]`, `/mode [id]` | model (or preset), reasoning effort or permission mode for the rest of the session |
| `/presets [new <name>]` | the presets with their rules, to pick one; `new` writes one to edit |
| `/resume` | go on with an earlier session in this project |
| `/undo`, `/retry` | take back the last prompt and what followed it, files too; `/retry` then sends it again |
| `/compact [focus]` | summarise earlier turns now, keeping `focus` in mind |
| `/status` | account and balance, model, mode, context use, what `/undo` restores, MCP servers, cost so far, session file |
| `/account` | balance and plan, the last `account.usage_days` of spend by model, balance fallback on or off, the account's keys (revoke any but this session's own) |
| `/copy` | the last reply to the clipboard, as markdown |
| `/init` | crowbot studies the project and writes its `AGENTS.md` |
| `/login`, `/logout` | pair this device or take an account number (shown masked), or forget the key |
| `/signup` | create an account in a card; its number shows once and its last digits are typed back |
| `/forums` | the forums the agent can browse: log in, log out, remove, add |
| `/quit` (or `/exit`) | leave |

`/login` and `/signup` take effect without a restart, and ask first before they would replace a
stored account number (signup asks about any stored key). A refused key in a session opens
`/login`; running out of funds or allowance says where the account stands. The project's `AGENTS.md` (else `CLAUDE.md`) joins the
system prompt at conversation start, up to `agent.instructions_bytes`. On Windows a paste arrives
as keystrokes; a key burst counts as a paste, so its Enters add lines.

## Presets

A preset is picked like a model (in `/models`, under the models, or with `--model <name>`), but
each request then runs on the model of the first of its rules that holds. It is a file,
`~/.crowbot/presets/<name>.toml`:

```toml
description = "Flash for subagents and when the plan runs low; the big model to plan"

[[rule]]
when = { agent = ["subagent"] }   # main, subagent, or a kind: explore, general
model = "glm-5.3-flash-crow"
effort = "low"                    # optional; else the session's /effort

[[rule]]
when = { allowance_under_pct = 15 }
model = "glm-5.3-flash-crow"

[[rule]]
when = { mode = ["plan"] }
model = "crow-2"

[[rule]]                          # no `when`: required last, it takes the rest
model = "deepseek-v4.1-flash"
```

Every condition a `when` sets must hold: `mode`, `agent`, `context_over` (tokens the last request
sent), `allowance_under_pct`, `balance_under_usd`. One crowbot cannot know yet (nothing sent since
a summary, an account not asked, no plan) does not hold. `/presets new <name>` (or the last row of
`/models`) writes a commented template on the session's model. The bar shows the preset and the
model that answered last (`thrifty → crow-2`), and the context share against that model's window;
each reply in the session file records its own model. A preset whose file will not parse, or that
names a model crowbot does not serve, is listed with why and refused when chosen. The system
prompt names the preset, so it stays the same across the requests; a model that caches loses its
cached prefix whenever a rule switches to another.

## MCP servers

crowbot can use the tools of MCP servers: programs it starts and speaks to over stdin and stdout,
or servers already running that it reaches at a URL over streamable HTTP. Most need no setup:
crowbot uses the servers your other apps already list (below). To name one yourself, put it in
`~/.crowbot/config.toml`, or in a trusted project's `.crowbot/config.toml`:

```toml
[mcp.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_PERSONAL_ACCESS_TOKEN = "…" }
tools = ["get_*", "search_*"]   # optional: offer only these, by the server's names
timeout_secs = 300              # optional: how long a call may take (else `mcp.call_secs`)

[mcp.ida]
url = "http://127.0.0.1:13337/mcp"
headers = { Authorization = "Bearer …" }   # optional, sent with every request
```

**Servers your other apps list.** Before a session, crowbot reads the MCP settings of Claude
Desktop, Claude Code, Cursor, Windsurf, VS Code, Cline, Roo Code, Codex, Gemini CLI, Copilot CLI,
LM Studio and opencode (`data/mcp_sources.toml`, a new app is a new entry). It asks once about
the servers it finds, listing what each runs and its secrets by name only (default yes). The
answer is kept in `~/.crowbot/mcp.json`: a no is never asked again, a server whose command or URL
changes is asked about again. A repository's own files (`.mcp.json`, `.cursor/mcp.json`,
`.vscode/mcp.json`, `.roo/mcp.json`) are asked about apart, with a warning, defaulting to no,
for that project only. A run that cannot ask (`-p`, `--json`) uses what was chosen and names the
rest on stderr. A server in crowbot's own config wins over one found with the same name, and
`mcp_discovery = false` turns the search off. `/status` names where a found server came from.

**IDA Pro, for example.** `ida-pro-mcp --install` writes its server into those apps' settings
(as a URL by default, or a program with `--transport stdio`), so crowbot finds it with nothing
more to set up. Start the plugin inside IDA first (Edit → Plugins → MCP), and answer yes when
crowbot asks.

A later layer changes one key of a server, or turns it off with `enabled = false`. Servers start
with the session, all at once, each within `mcp.start_secs`; one that does not is named once with
what it printed, and the run goes on. A project's servers start only once the folder is trusted;
until then they are listed without their `env` values. No server sees `CROWBOT_API_KEY`
(`data/hidden_env.toml`).

Their tools follow crowbot's own as `<server>__<tool>`, described as the server describes them,
and ask like bash: MANUAL asks (`permission = "mcp"`, `pattern = "github__*"`,
`action = "allow"` lets a server's tools run), AUTO runs them, and PLAN refuses them unless the
server marks a tool read-only (`readOnlyHint`), as `/undo` snapshots before any it does not. The
model is sent the text of an answer; images, audio and binary resources are named, not sent. A
call past its limit, or interrupted, is cancelled at the server. A server that stops fails its
calls at once and is said once; it is not started again. `/status` lists every server, how many
tools it offers, and whether it stopped. What a server says of how to use its tools joins the
system prompt under its name, up to `mcp.instructions_chars`. Resources, prompts, OAuth logins
and the legacy SSE transport are not supported yet (`grep -rn CEILING: src/mcp src/tools/mcp`).

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
| `src/agent/` | The run loop, tool batches, what the model is sent, subagents (`child.rs`, kinds in `kinds.rs`), and the session setup both frontends share. |
| `src/live/` | What a run shares with its tools and frontend while it is going: events, prompts, state, files read. |
| `src/conversation/` | Messages, parts and errors as a conversation holds them. |
| `src/tools/` | One file or folder per tool behind the `Tool` trait, registered in `tools/mod.rs`. |
| `src/permission/` | Rules (last match wins), the gate that layers them with the mode, the bash command split. |
| `src/forums/` | Forums: the directory, the sealed store, accounts and the Tapatalk backend. |
| `src/mcp/` | MCP servers: their config, JSON-RPC over a pair of line channels filled by a program or by streamable HTTP, the handshake, calls, and `discover/`: the servers other apps list and the choice to use them. Their tools as crowbot's are `src/tools/mcp/`. |
| `src/models/` | The model catalog (live, cached or bundled) and its table, presets and the rows they show as, and the choice of either by one name (`choice.rs`). |
| `src/commands/` | One file per command (effect-only ones share `fixed.rs`), registered in `commands/mod.rs`. |
| `src/frontend/` | What drives a run: `print` (headless) and `tui/` (the session), and the words and tool cards both share. |
| `src/text/` | Styled lines, theme, markdown, tables, highlighting, diffs. |
| `src/session/` | Append-only JSONL session files under `~/.crowbot/sessions/<project>/`: their entries, the tree reader that finds the current branch, the list `/resume` shows, undo. |
| `src/snapshot/` | The shadow git `/undo` restores files from. |
| `src/battery/` | The hidden `crowbot battery report`: a battery's runs counted by tool, shell command, stand-in, sequence and replayed gate verdict, then candidate changes ranked by count. |
| `src/config/` | Layered settings, locations, limits and the trusted-folder list. |
| `src/release/` | Release identity, install and self-update. |
| `src/*.rs` | Startup (`app`), routing (`cli`), the key (`auth`), data loading, efforts and modes. |
| `crates/cffetch/` | The user's Cloudflare-aware HTTP client, vendored unchanged (outside the workspace: see `Cargo.toml`). Re-sync: copy `src/`, `tests/`, `examples/`, `Cargo.toml`. Its networked suite runs from that directory. |
| `install/` | The install scripts each release publishes: `install.sh` (macOS, Linux) and `install.ps1` (Windows). |
| `tests/e2e/` | The real binary against an in-process fake crowbot; `pty/` drives real sessions. |
| `tests/fake_mcp/` | A stand-in MCP server, a test-only binary the scenarios configure as `{fake_mcp}`. |
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

A `verify` command (e.g. `verify = "make test"`) runs when a run that changed files would end; a
failure is handed back for the model to fix, capped so it cannot loop. It is off unless set, and
only your own or a trusted folder's config may set what crowbot auto-runs.

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
`v<version>-<run>` with one binary per target, `SHA256SUMS`, its signature `SHA256SUMS.minisig`
and the install scripts, built with that tag (`CROWBOT_RELEASE`) for `--version` and the updater.
Asset names are fixed, so `releases/latest/download/<asset>` (the Install lines) always serves
the newest. Adding a target: see `data/release.toml`'s header.

The signature's trusted comment is `crowbot release <tag>`, made with the repository secret
`MINISIGN_SECRET_KEY`; a release without it fails before building. The public keys are
`data/release.toml` `[signature] keys`. To change the key: release once with the new public key
beside the old (still signed by the old), then put the new secret key in place and drop the old
public key, since crowbots in the field know only the keys they were built with.

## Licence

crowbot is licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at
your option. Unless you say otherwise, a contribution you submit is licensed the same way.
