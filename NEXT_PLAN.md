# Next plan

Phases from the approved design. Each sub-step is checked as AGENTS.md says (`make lint test`;
e2e only for critical changes, once per batch).
Record what each phase taught us under **Learned**.

## M0 — Skeleton
- [x] Toolchain: rustup 1.98.1 (MSVC), GNU make 4.4.1 via winget
- [x] Reference repos and crowbot docs moved to `reference/` (gitignored); `git init -b main`
- [x] Cargo project, `clippy.toml` io-wrapper rule, Makefile, CI (3 OSes + static release builds), README
- [x] `data/` endpoints, limits, defaults, command specs, raven, models snapshot; parse tests per owner
- [x] `io/{http,fs,clock,term}`, `paths`, `settings` (defaults → user → project → flags), `limits`
- [x] `crowbot models` (fresh cache → live → stale cache → bundled snapshot), `crowbot help`
- [x] Fake crowbot (axum) + first e2e suite
- [x] `crowbot keytest` on Windows Terminal: Shift+Tab, Shift+Enter, multi-line paste, key release
- [x] CI green on all three OSes

**Learned**
- reqwest is 0.13: TLS features are `rustls` (aws-lc-rs provider, builds fine on MSVC without
  cmake) plus `rustls-platform-verifier` for OS roots. The ring/webpki plan item was moot.
- Static-CRT release build on Windows: 4.5 MB.
- Binary crates warn on unread `pub` fields, so data fields are added only when read.
- Windows Terminal via crossterm (no kitty enhancement): every key also sends a Release event
  (filter to Press/Repeat); Shift+Tab is `BackTab`+SHIFT; Shift+Enter keeps SHIFT; Ctrl+J is
  `Char('j')`+CONTROL. A paste is plain key presses (13 for "one⏎two⏎three", 2 of them Enter),
  all queued at once, never `Event::Paste`: the TUI must treat an Enter with more input already
  pending as a newline.

## M1 — Auth + streaming chat, headless
- [x] `api/sse.rs` with chunk-split fuzz tests
- [x] `api/wire.rs`, `api/assemble.rs`, `api/chat.rs`: reasoning echo, tool calls by index, usage + cost, error frames, request id
- [x] `data/errors.toml`, `api/retry.rs` (only before any output streamed; honours Retry-After)
- [x] `io/secret.rs` (DPAPI / 0600), `auth.rs`, `crowbot login` (pairing, `--key`, `--status`) / `logout`
- [x] `crowbot signup` (PoW on all cores, refetch on 428, refuses to replace a stored key)
- [x] `-p` / `--json`, session JSONL writer, stdin piping
- [x] `make smoke` (opt-in, real API, deepseek-v4.1-flash, asserts < $0.001)
- [ ] Real paired chat on Windows (needs the user to pair once)
- [ ] `data/overflow.toml` moved to M5 with compaction, where it is first used

**Learned**
- Tests must give the binary a null stdin: an inherited pipe that never closes looks like piped
  input and blocks. Real users hit the same with odd task runners; `< /dev/null` is the escape.
- Clippy rejects an enum variant named like its enum (`Stop::Stop`), hence `Finish::Done`.

## M2 — Tools + loop
- [x] Messages gain tool results and reminders; `context.rs` answers orphaned calls and trims cut-off turns
- [x] `agent/run.rs` loop: steer after tools, follow-ups at the end, turn limit, length-stop never runs calls
- [x] `agent/batch.rs`: clear in call order, run in parallel, results in call order; doom-loop guard
- [x] Tools: read, write, edit (+ `edit_match` cascade, 17 goldens), bash (Job Object / process group), glob, grep (embedded ripgrep), webfetch, codesearch, todowrite
- [x] Permissions: rules (last match wins), gate layering defaults → config → approvals → mode locks; `shell_split`, arity; mode × tool matrix test
- [x] Modes as data: MANUAL asks, AUTO allows everything (denies too), PLAN read-only with a plan file and reminder
- [x] Loop unit tests (steer, follow-up, reject, feedback, always, auto-approve, length, doom loop, plan) and 4 e2e scenarios
- [ ] A real run on the user's machine: `crowbot --mode auto -p "..."` against a scratch repo

**Learned**
- The shell tool that drives this repo turns `\\` into `\` inside heredocs; files with backslashes are
  written with the editor, never through a heredoc.
- Headless runs answer every prompt `Unavailable`, so the model hears why and carries on instead of
  the run hanging; the TUI will answer them for real.
- `Reply`/`steer`/`follow_up`/`set_mode` carry `allow(dead_code)` outside tests until M3 wires them.

## M3 — TUI core
Plan: inline renderer that commits finished blocks to real scrollback and diff-redraws only a live
region; Enter queues and Tab steers while running; prompts are a numbered card (Yes / No / No +
why, no "always"); tool cards are data (`data/tool_cards.toml`); `crowbot "…"` pre-types the prompt.
- [x] 3.1 text: styled lines, theme roles, markdown (+ stream split), syntect highlight, diff; snapshots
- [x] 3.2 screen: commit + live-region diff, vt100-tested (incl. 300 random frame sequences)
- [x] 3.3 input + editor: keymap data, paste bursts, editing/wrapping/history (release filter lands with the event stream in 3.5)
- [x] 3.4 blocks: welcome raven, transcript streaming (`tui/feed.rs`), data-driven cards, status, queue, footer
- [x] 3.5 app loop + session commands (`/quit`, `/mode`, scopes, effects) + PTY e2e (passes on Windows ConPTY too)
- [x] 3.6 prompts: one `Prompt` shape (permission, question, plan_exit), one numbered card (`tui/choice.rs`), "always" removed
- [x] 3.7 docs + manual Windows Terminal checklist
- [ ] Manual pass on Windows Terminal (by the user; the full-screen items are under M3.9):
  - raven renders; streaming commits without flicker or doubled lines
  - Shift+Tab cycles modes: MANUAL gray, AUTO purple, PLAN blue (both rules change colour)
  - the rule under the editor keeps model, effort (coloured), ctx and cost right-aligned at any width
  - `/` opens the command popup: it filters, ↑↓ move, Tab completes, Enter runs, Esc hides
  - `/login` pairs through the browser, and option 2 hides all but the last four digits
  - a multi-line paste stays in the editor
  - Enter mid-run queues, Tab steers; Esc interrupts a `sleep 100` and returns queued text
  - a MANUAL edit shows a diff card; 1 / 2 / 3 behave
  - resizing keeps the live region intact; Ctrl+D leaves the terminal clean

**Learned**
- syntect's bundled grammars have no TypeScript, TOML or PowerShell; `data/code_aliases.toml` maps
  what it can (ts → js) and the rest renders uncoloured.
- Mapping syntect scopes straight to theme roles (best `MatchPower` wins) keeps code colours in the
  same palette and makes highlighting snapshot-testable by role name.
- `make golden` must only rewrite expectations that fail, or it silently rewrites passing goldens.
- The new `text/` renderers carry a temporary `allow(dead_code)` until 3.5 wires the TUI (done).
- Windows ConPTY sends a cursor-position query (`ESC[6n`) and holds all output until it is
  answered; real terminals answer on their own, the PTY test harness answers it explicitly.
- All three prompt kinds fit one card, so there is no `View` trait (it would have one
  implementation); the `/` palette became Tab completion for the same reason.
- Switching to AUTO auto-approves only waiting *permission* prompts; questions still wait.
- The session loop needs no engine thread: it owns the transcript and moves it into each turn's
  future, which hands it back; controls mid-turn go through `&Shared`.

## M3.8 — Session polish (after first use)
Plan: mode colours as data (MANUAL gray, AUTO purple, PLAN blue); the message bar framed by two
rules in the mode colour, the bottom one carrying model + effort, ctx and cost right-aligned; a `/`
command popup; `/login` inside the session.
- [x] colours + `tui/frame.rs` (replaces `footer.rs`); effort levels carry a theme role
- [x] `tui/boxed.rs` shared by prompt cards and `tui/palette.rs` (the `/` popup)
- [x] live API key (`Arc<RwLock>` shared by clones), `api::pair::wait`, `auth::adopt`; login's words in its data file
- [x] `tui/login.rs`: pair or a masked account number; its network work is a future in the loop

**Learned**
- The Tab-only completion from 3.x was not discoverable; a popup is worth its ~150 lines once it
  shares the box with the prompt cards.
- Sorting matches alphabetically puts an exact name before longer ones (`/mode` before
  `/models`), so Enter never runs the wrong command.
- Anything the session waits on (a turn, a login) is a future beside input in one `select!`; a
  command awaited inside the input branch would freeze the screen.
- Opening the browser belongs in the job, not the state machine, or unit tests open real tabs.

## M3.9 — Full-screen session (after second use)
Plan: the session moves to the alternate screen with its own scrollable transcript so the bar can
never move and thinking blocks can be clicked; a crow working line; an aligned `/models` picker.
- [x] `Raw::fullscreen` (alternate screen, mouse, no auto-wrap); `screen.rs` diffs whole-screen rows
- [x] `feed.rs` keeps blocks as source (redrawn per width); `view.rs` scroll; `layout.rs` frame
- [x] thinking blocks: click one, Ctrl+T all; live header and tail while streaming
- [x] `status.rs`: wingbeat raven, user's verbs with a glint, time and tokens
- [x] `text/table.rs` for markdown, `crowbot models` and `tui/picker.rs`; model switch per session
- [ ] Manual pass on Windows Terminal (by the user):
  - the bar never moves: popup, `/login`, a permission card, at the top and after long output
  - wheel and PgUp/PgDn scroll; `↓ N more` shows while scrolled up; the end follows again
  - clicking a thinking block opens it; Ctrl+T toggles all; resizing re-wraps everything
  - the raven flaps and the verb glints; `/models` → ↓ → Enter switches the model
  - Shift+drag selects text; quitting returns to the shell with the "Session saved" line

**Learned**
- The inline renderer drifted when the live region outgrew the free rows: the terminal scrolled
  but the renderer's count of used rows did not. Full-screen frames at absolute positions, with
  auto-wrap off, cannot drift at all.
- Mouse capture and inline scrollback cannot coexist: the wheel goes to the app. Clickable
  blocks meant owning the scroll.
- Kitty keyboard flags are kept per screen, so they are pushed after entering the alternate
  screen and popped before leaving it.
- ConPTY forwards SGR mouse reports as console mouse events, so clicks are testable end to end.
- Shrinking every column evenly makes a narrow table useless; dropping low-priority columns
  first (data) keeps it readable.
- A popup that grows the bar shrinks the transcript's viewport, so the whole conversation jumps
  up and reads as a cleared screen. Floating cards overlay the rows above the bar instead.

## M3.10 — webfetch on cffetch
Plan: webfetch fetches through the user's cffetch (browser TLS via wreq/BoringSSL, another
validated profile when one is challenged, challenge pages told apart from real ones) so protected
pages load, and pages that still need a browser fail with a clear message instead of the
interstitial passed off as the article. The API stays on reqwest.
- [x] `crates/cffetch/` vendored unchanged, excluded from the workspace
- [x] `io/fetch.rs` (`Fetch` in `App`, replacing `App.http`); `CfError` → `FetchError`
- [x] webfetch on `Fetch`; its messages in `data/tools/webfetch.toml`; the model told not to retry
- [x] e2e: the real binary and client against a readable page and a Cloudflare-style challenge
- [x] CI: `.github/actions/setup` on every job; Linux ships static musl
- [x] First CI run: the toolchain builds on all three OSes and `release-musl` passes, so Linux
      ships fully static
- [ ] Manual: webfetch on a normal docs page and on a known protected page in a session

**Learned**
- A vendored crate must not be a workspace member: cargo lints and formats every member it
  builds, whatever `default-members` says. `exclude` makes it an outside crate (`--cap-lints`),
  and `cargo fmt` without `--all` leaves it alone.
- BoringSSL honours `+crt-static` on its own (btls-sys passes `/MT` and the MultiThreaded
  runtime to CMake); the static binary still imports only system DLLs. It costs about 6.4 MB.
- CMake finds NASM on `PATH` only; the `NASM_PATH` variable cffetch's own config set does not
  reach it.
- cffetch over plain HTTP against the fake exercises the whole path (binary, tool, client,
  detection) with no real Cloudflare, so the challenge case is a hermetic e2e test.
- BoringSSL builds against musl too, so the Linux release is the static musl binary and glibc
  stays the CI check target.

## M4 — Account UX in the TUI (planned 2026-10-16)
Plan: the session shows where the account stands and handles every account step itself. Signup
becomes a card. The bar carries the balance and the plan's allowance. The welcome follows the
key. A rejected key offers the login card, and running out of funds says where the account
stands. Login asks before it replaces a stored account number. An `/account` card lists and
revokes keys, sums recent usage and toggles balance fallback. Buying plans, payments and deleting
the account stay on the web. Each new `Me` field, endpoint or fake route lands with its first
reader, since unread fields fail `-D warnings`.
- [x] 4.1 standing: `Me` reads `pass` and `spend_balance_when_dry` (optional; the two-field reply
      still parses); one `standing()` worded in `data/commands/account.toml`, used by login,
      `/status` and a first `/account`; the fake's `/api/me` is scriptable
- [x] 4.2 bar: balance and allowance items (`data/ui.toml [footer]`), refreshed at start, on a
      key change (a counter beside the key in `Api`), at run end and at most every
      `tui.account_refresh_secs` mid-run; never on an idle timer
- [x] 4.3 welcome: replaced in place when the account changes; says not logged in, key file
      unreadable, key rejected or cannot spend
- [x] 4.4 login: asks before replacing a stored account number or an unreadable key file; its
      errors go through `data/errors.toml` like the feed's
- [x] 4.5 errors say what follows (`session` in `data/errors.toml`): a 401 opens the login card,
      a 402 or a spent allowance refreshes the account and shows its standing; the key is never
      dropped on a 401
- [x] 4.6 signup core shared by `crowbot signup` and the session: the number is saved before
      anything can drop it, and the proof of work stops when its job is dropped
- [x] 4.7 `/signup` card: asks before replacing a stored key, shows the number, confirms its last
      four digits
- [x] 4.8 `/account` card: overview and keys; revoke any key but the session's own
- [x] 4.9 `/account`: the last `account.usage_days` of usage by model, and the balance fallback
      toggle
- [x] 4.10 README and Learned
- e2e (auth and billing flows): one local batch after 4.5 (47 passed), one after 4.9 (51 passed).
- [ ] Manual, against the real API with a scratch account: the bar's allowance on a plan, `/account`
      usage, a fallback flip and a device key revoked (all tested against the fake only)

**Learned**
- Unread fields fail `-D warnings`, so the order held only because each field, word and route
  landed with its first reader: the replace question's device wording left 4.4 and came back in
  4.7 with signup, its first user.
- A counter beside the key in `Api` lets the session notice `/login`, `/logout`, `/signup` and
  `forget` without any of them telling it, and lets a refresh answered for an old key be dropped.
- A card's prebuilt body (the replace question, `/account`) wraps at the width it opened at, as
  prompt cards do; a resize does not rewrap it.
- A key refused while login checks it was never in use, so "it may have been revoked" misled:
  it is `key_unknown` now. crowbot's `plan_allowance_spent` is a 402 like an empty balance, and
  `/v1` never reports the balance, so the bar asks `/api/me` (CEILING in `tui/app/account.rs`).
- pty tests must close a card they leave open, or Ctrl+D goes to the card and the session never
  quits; and a sentence a card wraps can only be matched by a piece that stays on one row.

## M5 — Context + sessions (compaction, shadow-git /undo) (planned 2026-10-16)
Plan: a session can be continued, taken back and kept inside the model's window.
`crowbot --continue` and a `/resume` picker reopen this project's sessions with their model,
effort and mode. `/undo` takes back the last prompt, the conversation as a branch the file keeps
and the files from a shadow git snapshot, and `/retry` sends it again. Long conversations are
summarised before the window fills, by `/compact` or on their own, and a request refused as too
long is compacted and sent once more. The format stays v1: new line types and optional fields
only, and a running head decides the branch.
- [x] 5.1 session tree: the reader follows parents and `rewind` markers to the current branch;
      64-bit ids; a frozen fixture with branches
- [x] 5.2 `crowbot --continue` (TUI and headless): reopen the latest session, its recorded
      model, effort and (in the TUI) mode, and the conversation replayed
- [x] 5.3 `/resume`: a picker over this project's sessions, the `/models` picker made one table
      picker
- [x] 5.4 `/undo`, the conversation: a `rewind` marker, the prompt back in the editor
- [x] 5.5 shadow git (`io::proc::exec`, `src/snapshot/`): a snapshot before a prompt's first
      change, files restored on `/undo`; only inside a git work tree
- [x] 5.6 files changed after the turn are left alone and named; `/status` says whether `/undo`
      restores files
- [x] 5.7 `/retry`: undo, then the prompt again as a sibling branch
- [x] 5.8 `/compact [focus]`: a summary of all but a recent tail (`compaction.keep_tokens`),
      sent as a reminder so the system prompt never changes
- [x] 5.9 automatic compaction before any request past `compaction.threshold_pct`;
      `data/overflow.toml`; a refused request compacted and sent once more
- [x] 5.10 README and Learned
- e2e (session format, file restore, requests): one local batch after 5.3 (54 passed), 5.7 (57)
  and 5.9 (58); CI passed on the push after 5.7, Windows and macOS included.
- [ ] Live: one real too-long request (or the gateway's own wording) to confirm
      `data/overflow.toml`'s phrases; only crowbot's 413 is tested end to end
- [ ] Manual on Windows: `/undo` in a real repository (CRLF, long paths, a file open elsewhere)

**Learned**
- A running head made branches free: a line without a known parent follows the head where it was
  written, so every v1 file reads exactly as before and the format stayed v1. Three line types
  (`rewind`, `snapshot`, `compaction`) and one optional field (`state`) were enough.
- A summary is weighed only against replies made since it: the big reply just before a summary
  would otherwise ask for another one at once.
- The summary request keeps the session's system prompt and tools, so its prefix is the one the
  vendor has cached; it is the conversation plus one instruction, not a new conversation.
- git needs the shadow's folder to exist before `--git-dir` can init it, and a user's global
  `core.hooksPath` would run their hooks on a restore; hooks point at the shadow's own empty
  folder. Using the repository's top level as the work tree is what makes `.gitignore` apply.
- The fake's errors say "fake {kind}", so overflow by wording is tested against the run harness,
  and only the 413 crowbot answers itself goes end to end.
- A pty test that leaves a prompt in the editor (undo puts it back) must clear it before Ctrl+D
  can quit.
## M6 — Subagents + MCP (planned 2026-10-16)
Plan: the model can hand a self-contained job to a subagent, and use the tools of MCP servers the
user configures. A `task` tool runs a child loop of one kind from `data/agents/` (`explore`
read-only, `general` with the parent's tools) under the parent's mode; its answer is the tool
result, its cost counts in the parent's, its conversation is a file beside the parent's, and its
edits are the parent prompt's for /undo. MCP servers are local programs in `[mcp.<name>]`, started
with the session (a project's only once trusted); their tools join the registry as
`<server>__<tool>` and ask like bash: MANUAL asks, AUTO runs, PLAN refuses all but read-only ones.
- [x] 6.1 the registry holds owned specs, built by adding; each description filled once
- [x] 6.2 the run harness moves to `src/agent/testing.rs`
- [x] 6.3 `task` + `explore`: a child runs in its own file and answers
- [x] 6.4 a child's cost counts in the parent's, live and after resume
- [x] 6.5 the task card, headless notes, `child` events in `--json`
- [x] 6.6 parallel children within `subagent.parallel`
- [x] 6.7 `general`: the parent's tools; its edits are covered by /undo
- [x] 6.8 a plain No inside a child stops the parent
- [x] 6.9 README and Learned (subagents)
- [x] 6.10 trust lists a left-out table entry by entry, secrets by name only
- [x] 6.11 `[mcp.<name>]` servers start with the session (`io::proc` child, JSON-RPC, handshake);
      /status names them
- [x] 6.12 their tools join the registry (`<server>__<tool>`, optional `tools` allowlist) and ask
      as `mcp` / `mcp_write`
- [x] 6.13 read-only tools (`readOnlyHint`) run in PLAN by the rules and take no snapshot
- [x] 6.14 interrupts send `notifications/cancelled`; a server that stops is reported once
- [x] 6.15 README and Learned (MCP)
- e2e: one local batch after 6.5, 6.8, 6.12 and 6.14; each batch is pushed so CI covers Windows.
  Subagents: 61 after 6.5, 63 after 6.8. MCP: 64 after 6.12, 65 after 6.14.
- Open (live): a real npx server on Linux and Windows (a `.cmd` shim started directly); the
  gateway taking real-world schemas such as GitHub's; a battery re-run against the baseline, since
  `task` and `system.md` changed it.

**Learned (subagents)**
- A subagent is the same loop under its parent: a `Shared` of its own whose cancel token is a
  child of the parent run's, with the parent's prompts and mode shared and its own queues, read
  set and reminder latch. Nothing in the loop had to learn it was nested; the run future proved
  `Send` behind the tool's `BoxFuture` without a change.
- One `child` event wrapping the subagent's own keeps the stream additive and settles a real
  collision: subagents reuse call ids like `c1`, so frontends keep their lists for the outermost
  call only.
- An interrupt must wait for a subagent, not drop it (`Tool::stops_on_cancel`), or what it
  spent and its last turn are lost.
- Esc at a permission card is a No, not an interrupt; inside a subagent that No now stops the
  parent, which is what makes a declined child card behave like one of the session's own.
- One snapshot per prompt, shared through the root run's `OnceCell`, is what lets parallel
  subagents edit without two `git add -A` on one shadow index.

**Learned (MCP)**
- The RPC dispatch keeps a sender to answer pings, so dropping the client never closed the
  server's stdin and every stop waited out its grace, then killed. A stop closes stdin itself;
  only a test that stopped a server with a sender alive showed it.
- A program that dies closes stdout and stderr in no set order, so its stderr tail read when the
  stream ends can come back empty. The tail waits, bounded, for stderr to close: the start notice
  had the same race and passed by luck.
- A tool left out over a name clash, said through "is not running", showed the `mcp` event must
  carry the whole sentence from the side that knows what happened.
- `[[expect.tools]]` with exact names would tie every MCP scenario to the built-in list; `has` and
  `lacks` keep each about its server. A fake that logs what it heard into the project, read back
  with `expect.files`, proves the cancel without a test harness of its own.
- `make lint | grep` hides lint's exit status: one batch passed that way while lint failed.
## M7 — Release (`prod` → GitHub Releases, installers)
- Added 2026-10-02: every push to `prod` runs `ci.yml` through `release.yml` and publishes
  `crowbot-<target>` for musl Linux, Apple-silicon macOS and Windows, plus `SHA256SUMS`. Proven
  2026-10-06 by `v0.2.0-2` through `v0.2.0-4`.
- Added 2026-10-03: the repository is public (MIT OR Apache-2.0, history rewritten to a noreply
  author and without cffetch's live-site probes); release builds carry their tag; `crowbot update`
  and a daily background update for interactive release builds (SHA256SUMS-checked, the program
  swapped where it stands); `crowbot install` / `uninstall`, and on Windows a double-clicked
  release offers to install itself (user PATH in the registry, Apps & Features entry).
- Planned 2026-10-16: releases sign `SHA256SUMS` with minisign and the updater refuses a release
  whose signature fails; `install.sh` and `install.ps1` go out with every release, so one line
  installs crowbot. Decided: minisign only (notarisation and Authenticode need paid accounts);
  the scripts live here as release assets, and the website's docs show their one-liners.
- [x] 7.1 the updater checks a minisign signature on `SHA256SUMS`, its trusted comment naming the
      release (`data/release.toml` `[signature]`, a list of keys)
- [x] 7.2 the release signs `SHA256SUMS` with a key kept as a CI secret
- [x] 7.3 `install/install.sh` and `install/install.ps1`, published with each release
- [x] 7.4 README and the website's docs name the one-liners and the signature
- [x] 7.5 a release, on an explicit go; the signature and the one-liner checked live
- e2e: one local batch after 7.3, then pushed so CI runs the scripts on macOS and Windows.
- 2026-10-16: `v0.2.0-6` (deploy 8477a0f) is the first signed release. Its `SHA256SUMS.minisig`
  verifies with the minisign CLI (trusted comment `crowbot release v0.2.0-6`), and the published
  `install.sh | sh` installed `crowbot 0.2.0 (v0.2.0-6)` into a throwaway home. The release ran
  its checks on every target (macOS 474 + 64, Windows 464 + 63 tests). The website's docs
  (crowbot ba068b2) show the one-liners; its deploy is a separate go.
- Open: the updater's signature check runs for real only on the release after this one (older
  installs do not check); notarisation and Authenticode (paid accounts); Intel macOS, arm64
  Linux and Windows on ARM targets.

**Learned**
- From 91c1b39 (2026-10-08) to efbb769 every CI job only built: `if: matrix.checks != false`
  is false for an entry without `checks`, as Actions compares null and false as 0. Every green
  run of M5, M6 and M7.1–7.3 tested Linux only, on this machine; "CI covers macOS and Windows"
  in M5's and M6's notes was not true until 9964060. A flag now names what to skip
  (`build_only`), and a run counts as tested only once its job logs show `test result` lines.
- What the eight days hid, all fixed: /undo on macOS and Windows snapshotted the shadow's own git
  dir and a restore deleted its objects (crowbot's home inside the work tree was compared
  unresolved against git's resolved path); a test file name Windows refuses (`*`); an unreadable
  key file's reason cut off by a long macOS temp path; pty tests sending /undo before the run had
  ended, or an Enter read with its text as a paste on a loaded Windows runner; two
  platform-only lint errors.
- The minisign crate writes raw key bytes from `generate_and_write_*`; the CLI's file format is
  `to_box().into_string()`. The key was proven with Ubuntu 24.04's minisign 0.11, the runner's,
  unpacked without root.

## M8 — Forums (Tapatalk/mobiquo)
- Added 2026-10-03: crowbot browses forums that run the Tapatalk plugin. Transport rides
  `io::fetch` (cffetch's browser TLS, so a Cloudflare WAF that blocks reqwest lets it through);
  `src/forums/` holds a lenient XML-RPC codec (tolerates the server's dropped `</member>`), the
  mobiquo backend, Tapatalk directory discovery/resolve, and a sealed store seeded with
  UnknownCheats. Agent tools: `forum_sections` / `forum_topics` / `forum_thread` / `forum_search`
  (read, default-allow) and `forum_post` (reply or new topic, confirmed every time even in AUTO,
  refused headless). `/forums` and `crowbot forums` add, log in (hidden password, never stored),
  list, logout and remove.
- 2026-10-04: `/forums` is a card (`src/frontend/tui/forums.rs`): list, per-forum menu, username +
  masked password login, and an add flow (unadded `[[suggest]]` searches or free text → directory
  results). Its work runs in the one card job slot, so closing the card drops what it waited on.
  e2e: a pty test logs in to a fake forum, wrong password first, and checks no file holds the
  password.
- Unit-tested at every layer against in-process fakes (transport, codec incl. malformed XML,
  backend, directory, store, tools, commands). Protocol facts come from a live capture; not yet
  re-run live.
- Open:
  - a live smoke run against UnknownCheats: confirm `search`/`reply_post`/`new_topic` param shapes
    and that cffetch clears the WAF (the mobiquo dir can rotate — re-resolve on a 404);
  - an e2e scenario driving a forum tool through the agent loop (a store seeded the way the
    `/forums` pty test seeds it, plus fake mobiquo routes for the read calls);
  - a live run of the card's directory search (the add flow is unit-tested against fakes only);
  - prompt-injection hardening of forum content, cookie-expiry refresh, per-forum rate limiting,
    and more backends (Discourse's JSON API, a plain-HTML reader) behind the same `kind` switch.

## M9 — Tool-use battery (planned 2026-10-05, started 2026-10-12)
- Why: the basic tools (grep context/files/count, multi-path read, read depth, `fs`) and PLAN's
  allowlist were chosen from habit, not data. Run a large set of realistic sessions against the
  real model, record every tool call, and let the counts say which patterns to make faster or
  easier: shell commands the model keeps writing, scripts it writes for basics, calls refused or
  repeated, steps that always come in the same order.
- Decided 2026-10-12: `glm-5.3-flash-crow` at its default effort (~$0.085 a task); 30 tasks × 3
  repeats under a $60 cap; `dtolnay/semver`, `more-itertools` and `unjs/ufo` pinned by commit plus
  the `calc` fixture; MANUAL measured by replaying the gate on recorded asks, not by a headless
  approve policy. The caps live in `bench/battery.toml`, not `data/limits.toml`: the binary never
  reads them.
- [x] Commands the model runs no longer inherit `CROWBOT_API_KEY` (`data/shells.toml` `unset`),
      so neither a battery run nor a user's session can leak the key into a session file.
- [x] Tool results record `asks` and `refused` in the session file (never sent to the model).
- [x] Corpus: `bench/repos.toml`, 30 `bench/tasks/*.toml` (6 PLAN), 7 planted-bug patches, two
      hidden checks.
- [x] Runner: `tests/battery/` (`make battery`, `make battery-setup`), tested against the fake.
- [x] `make battery-setup`: every repo's suite passes untouched, every planted bug fails (96 s).
- [x] Analyzer: a hidden `crowbot battery report <dir> [--against <dir>]` over the session
      files and results: calls per tool; bash by first word, first two words and flags; pipe filters; `python -c`,
      heredocs, awk and sed programs (the scripts-for-basics smell); refused and declined calls,
      retries and doom loops; the most common call sequences (n-grams); turns, tokens, cost and
      time per task; task success from the check; MANUAL and PLAN verdicts replayed on the asks.
- [x] Pilot (`make battery TASKS=… REPEATS=1`, 3 runs, $0.04), then the full run and a report:
      `bench/runs/20261012T105806` (local only), 90 runs in 81 min, $1.89, 69 done, 14 ended on
      a server error. The candidates are below, each with the count behind it.
- [x] Re-run to show the applied candidates moved the numbers: `bench/runs/20261016T085230`
      (local only; crowbot 506f196), 90 runs in 86 min, $2.78, 87 done, none ended on an error.
      Its `report.md` and `results.jsonl` are the new `bench/baseline/`; the first full run's
      stay in git history. A later run is compared with that one by hand. On a fresh machine
      (Linux or macOS:
      the runner stops runs with SIGINT, and ufo's setup links with `ln -s`):
      1. Rust from `rust-toolchain.toml` plus libclang, git, python3, and node 18.12 or later with
         corepack (ufo's pnpm refuses older).
      2. The key in `~/.config/crowbot-smoke.key` (mode 600), never in the repo.
      3. `make battery-setup` (about 2 min, free).
      4. `CROWBOT_SMOKE_KEY=$(cat ~/.config/crowbot-smoke.key) make battery` (~$2; about 2.5 h at
         `parallel = 2`, which must not exceed the requests the key's plan runs in flight).
      5. `cargo run -q -- battery report bench/runs/<stamp>`, then diff its `report.md` with
         `bench/baseline/report.md`.
      A run beside ongoing work goes in a detached `git worktree` of the measured commit; it builds
      its own `target/` (a few GB), so remove the worktree once the report is written.

**Learned**
- A run costs about 2¢ with this model, not the 8.5¢ estimated: the whole battery is under $2.
- 14 of 90 runs ended on crowbot's errors, not the model's: 7 × HTTP 502 that outlasted crowbot's
  retries, 4 × `upstream_error: incomplete response` and 2 × `error decoding response body`
  mid-stream (not retried: output had already streamed), 1 × 503 "no ready backend". The report
  now counts them apart (`## Ended on an error`). Without them AUTO finished 63 of 65; PLAN
  finished 6 of 18, with 5 stopped by the 15-minute wall and 7 ended on an error.
- One stream stopped mid tool call for 891 s and the 120 s idle timeout never fired; crowbot's
  keepalives probably kept arriving after upstream had stopped.
- codesearch timed out at 30 s on 14 of its 21 calls. It uses `http.request_timeout_ms`, meant
  for small account calls; a search that succeeded took up to 16 s.
- The model looks answers up: 15 runs over 9 tasks called codesearch or webfetch on the upstream
  repos, e.g. webfetching the pinned `src/eval.rs`. AUTO allows every call, so fix-task success
  overstates what the model works out alone; private fixtures would close that.
- `shell_split` read a heredoc's body as commands, so MANUAL listed `for x in y:` and `}` as
  patterns to approve, and a quote in the body could hide the commands after it. The body is now
  skipped; `bash_complex` still asks about the whole line. This battery's asks predate the fix, so
  its replay tables still list body lines (`for`, `}`, `if`, `#`).
- The prompts already say what the model ignores: 54 `cd <project>` although `<env>` names the
  working directory, 33 `ls` and 29 `grep` in bash against bash's own description, and in PLAN
  11 of 27 refused calls are file readers the PLAN reminder says are refused. More prompt text is
  unlikely to move these; the gate and the tools have to meet the model where it is.
- Setup: the cached repos must be excluded from crowbot's Cargo workspace, and pnpm through
  corepack stops to ask unless `COREPACK_ENABLE_DOWNLOAD_PROMPT=0`.
- semver's copies shared one `CARGO_TARGET_DIR`, and `git archive` stamps every file with the
  commit's time, so cargo took another copy's later build as fresh: a second setup failed untouched
  on the tilde bug's binary. Each copy now builds its own `target/`. In the baseline, a cargo check
  or the model's own `cargo test` could have run a parallel run's binary, so its semver rows are
  suspect, fix-caret's one failure most of all.
- The first re-run (`20261014T090725`) is void: the smoke account's plan runs 2 requests in flight
  and the battery ran 4, so 84 of 90 runs ended on a 429 after crowbot's four 2 s retries.
  `parallel` is now 2.
- The re-run (2026-10-16) against the first full run: 87/90 done (69), none ended on an error
  (14), PLAN 16/18 (6/18), no run stopped by the 15-minute wall (9), $2.78 and 14.5M tokens
  ($1.89, 10.1M) because runs that ended early now finish. None of the 502s, 503s or cut-off
  streams came back, which is crowbot's gateway as much as this client, so the run says nothing
  new about retries or the stall timeout.
- The candidates moved what they aimed at: under MANUAL `cd` (54), `ls` (33), `git log` (19),
  `echo` (16) and `git status` (7) no longer ask, PLAN refused no `git` (21), and heredoc bodies
  no longer show as commands to approve.
- codesearch's lookups moved to webfetch: 16 calls became 53, 17 of them failed, and MANUAL would
  ask before each. The model still reads the pinned repos upstream, so private fixtures remain the
  way to measure what it works out alone.
- Readers as reads half landed: PLAN still refused 12 grep lines, 6 of them `"a\|b"`
  alternations that the reader check refused for the backslash inside double quotes. Fixed after
  the run (de7b042), not yet measured; `2>/dev/null`, `|| true`, `-r` and a pipe into `awk` keep
  the rest ordinary commands by design.
- The three runs not done are the model's: `ufo-fix-double-slash` #3 stopped on a `length` finish
  in its first reasoning (1 turn, 1.3 s), and two `semver-plan-intersects` plans never named the
  file the check looks for.
- A pinned worktree's own `target/` beside the main one filled the one 98 GB partition as the run
  ended. Dev builds now keep line tables only (9 GB to 3.1 GB).

**Candidates** (counts across the first full run's 90; all five applied 2026-10-12 and measured
2026-10-16, above)
- [x] codesearch timed out on 14 of 21 calls at 30 s, and the rest were upstream lookups: removed
      rather than given its own timeout; webfetch covers what it was for.
- [x] `cd <project> &&` and `git -C <project>` run where commands run anyway
      (`data/workdir.toml`): 54 `cd`s asked about under MANUAL, and 11 of PLAN's 27 refused bash
      calls were `git -C <project> log/status`, which PLAN allows without the `-C`.
- [x] MANUAL allows PLAN's inspection by default, both from `data/command_lists.toml`: `cd` 54,
      `ls` 33, `git log` 19, `echo` 16 and `git status` 7 of its asks. Tests (`python -m pytest`
      58, `cargo test` 48, `npx vitest` 21) run project code and stay asks.
- [x] `cat`, `head`, `tail`, `wc` and `grep` on named files ask as reads of those files, so PLAN
      runs them when the read rules allow: 11 of its 27 refused calls. `sed` and `awk` stay out,
      since both can write or run commands.
- [x] Each reply event must come within `chat.stall_timeout_secs` (300 s), keepalives or not:
      one run lost 15 minutes.
- Later: PLAN allows `npx vitest` and `python3 -m unittest`; edit's "read it first" refusal fired
  6 times after grep had shown the lines; read takes `path:line` (once). read→read→read (54, then
  83 in the re-run) is mostly reads that depend on the one before, so it is not a batching
  candidate yet. Under MANUAL, webfetch (53) and project test runs (`python` 54, `cargo test` 39)
  are now the most asked; both run or fetch what a person should see, so they stay asks.

## M10 — MCP servers found instead of configured (planned 2026-10-17)
Plan: crowbot uses the MCP servers the user's other apps already list (Claude Desktop, Claude
Code, Cursor, VS Code, Codex, … in `data/mcp_sources.toml`), asking once, and talks to servers
over streamable HTTP as well as stdio, so `ida-pro-mcp --install` alone makes IDA's tools
available. Decided: HTTP is in (IDA's installer writes HTTP entries by default); servers from
the user's apps are asked about once, default Yes, No never asked again, a changed command asked
again; a repository's own files (`.mcp.json`, `.cursor/mcp.json`, `.vscode/mcp.json`) are asked
separately, default No, per project.
- [x] 10.1 tests never read the developer's own apps: each run gets its own HOME, USERPROFILE,
      APPDATA and XDG_CONFIG_HOME; scenarios gain `user_files`
- [x] 10.2 streamable HTTP servers (`[mcp.x] url = …`): `io::http` response headers, the SSE
      parser shared, `src/mcp/http.rs` filling the same line channels
- [x] 10.3 where other clients keep their servers: `data/mcp_sources.toml`, one reader per shape,
      JSONC and TOML, names made to fit, precedence, fingerprints
- [x] 10.4 asked once, remembered (`~/.crowbot/mcp.json`), before a session; headless uses what
      was chosen; `/status` names the source
- [x] 10.5 a server's `instructions` reach the model in the system prompt
- [x] 10.6 README and Learned
- e2e: one local batch after 10.2 and 10.4, each pushed; CI logs checked for `test result` lines.
  67 after 10.2, 69 after 10.4 (the scenarios count as one test).
- Open (live, on a machine with IDA Pro): `ida-pro-mcp --install` over HTTP and with
  `--transport stdio`, the question, and the gateway taking its tools' schemas.

**Learned**
- IDA's installer writes a streamable-HTTP entry by default, not a program: discovery alone
  would have found it and failed to reach it, so HTTP came first. Its server answers
  `initialize` with the version asked and replies in plain JSON with a session header.
- Most apps share one shape (`mcpServers` → `command`, `args`, `env` or `url`); VS Code
  (`servers`, JSONC), Codex (TOML) and opencode (`command` as one list) differ. Writing every
  path as an env template, with an unset variable skipping it, let one list serve all three
  platforms with no `os` field, and lets tests move every path into a sandbox.
- A choice is keyed by a fingerprint of what a server runs or reaches, not of its secrets: a
  rotated token asks nothing, a new program asks again.
- Unread code fails `-D warnings`, so reading the sources (10.3) could not land before the
  asking (10.4); they went in as one commit, built and tested in order.
- `clippy::large_enum_variant` fired only on Windows, where a process holds its job handle.
- Windows CI, now running its tests, found three more: a refused local connect is retried for
  seconds there, so io::http called it a timeout ("could not connect" now comes first, with its
  cause); the bash tool drained its pipes for 250 ms after the shell exited before killing what
  it left, and a loaded runner lost the last output (leftovers are killed first now, and the
  pipes read to their end); a stop test timed PowerShell's cold start. CI green on all four
  targets at fb1f67d, its logs showing the discovery tests on Windows and macOS.
- CEILINGs: no OAuth, legacy SSE, GET stream or re-handshake after a server restarts; Claude
  Code plugins and Claude Desktop extensions not read; one answer per question; the 2026-07-28
  protocol drops `initialize` for `server/discover`, which crowbot does not probe yet.

## M11 — MCP battery (planned 2026-10-17)
Plan: measure how a cheap model uses MCP tools in crowbot, and what to change where it does not.
Simulated servers complex enough to trip a model (paged lists, required fields, enums, hex
addresses, open-first state, long outputs, a transient failure), tasks that need them, and report
sections about them, on M9's runner. Decided: `deepseek-v4.1-flash` for the full run plus one
repeat with `glm-5.3-flash-crow` as the reference; four servers (reverse engineering modelled on
ida-pro-mcp, an issue tracker, a database, a docs search), about 12 tasks × 3, at most $5.
- [x] 11.1 a data-driven simulated server: `crowbot-fake-mcp --server <file> --record <dir>`,
      arguments checked against the tool's own schema, every call logged with its outcome
- [x] 11.2 four simulated servers in `bench/mcp/`
- [x] 11.3 MCP tasks in the battery runner: suites in `bench/battery.toml`, a task's `mcp`
      servers in the run's config, `.mcp/` kept with the run, checks on the recorded state
- [x] 11.4 MCP in the report: outcomes per server and tool, mistakes, one call per reply,
      repeats, candidates
- [x] 11.5 the runs: a pilot, the full run, the reference repeat; `bench/mcp-baseline/`
- [x] 11.6 the findings, ranked, to choose fixes from: the user chose all four (2026-10-17)
- [x] 11.7 the system prompt names the connected MCP servers and says to answer from their tools
- [x] 11.8 a reply that writes tool calls as text is asked again once, to make them as calls
- [x] 11.9 the report's sequences step by reply, so calls made together are one step
- [x] 11.10 a headless run's prompt says no one will confirm, so make the change
- [x] 11.11 a re-run with deepseek-v4.1-flash against `bench/mcp-baseline/`

**Runs (2026-10-17, local only in bench/runs/)**
- Pilot `20261017T125927`: 2 tasks, both done, $0.0005.
- First full run `20261017T130025` (deepseek-v4.1-flash, 39 runs, $0.0143, 33 done) showed four
  faults of the battery's own (a check that read a comment already on the issue, an address
  lookup that matched only a function's start, missing references, a busy retry ranked a
  mistake); mended in 6a8807e.
- Clean run `20261017T131132` (deepseek-v4.1-flash, 39 runs, $0.0150, 34 done, 4 ended on API
  502s): the baseline, `bench/mcp-baseline/`.
- Reference `20261017T132430` (glm-5.3-flash-crow, 13 runs, $0.0737, 13 done): 6.9 calls per
  run against flash's 11.7, at about fifteen times the cost.

**Learned**
- The weak model sent no argument a server refused in 78 runs: required fields, integers, enums
  and `0x` addresses all as the schemas said. Checking arguments in crowbot before sending
  buys nothing yet; the CEILING stays.
- It batches unprompted: 78 searches and 50 issue reads went out several to a reply, against a
  handful made one reply at a time. A server's "busy, try again" was retried as asked.
- Its failures are rarer and of another kind, each 1–2 in 78: answering at once without a tool,
  inventing what "MCP" stands for (the server's instructions were in the prompt); tool calls
  written into the reply as text, which ends the run; listing what it would change instead of
  changing it; a prototype copied inexactly (`struct` dropped).
- A simulator must behave as its description says, or the battery measures the simulator: the
  address lookup and the missing references cost 8 lookups in the first run.
- Check what a check selects: "every selected item passes" over a collection that already held
  data failed two runs that did the task.
- API 502s ended 4 of 39 runs, as in M9; not an MCP matter.

**Candidates, ranked**
1. When MCP servers are connected, say so in the system prompt in plain words (their names,
   that their tools answer questions about what they hold, never to answer from memory): the
   two runs that answered without a tool.
2. A reply whose text holds tool calls (`{"tool_calls": …}`) is asked again once, telling the
   model to make them as calls: one run ended this way.
3. The report's "in a row" sequences count calls already made together in one reply as a
   candidate for one call; they should split at reply boundaries (report only).
4. A headless run that ends listing changes it did not make (one run): a line in the headless
   prompt that no one will confirm, so act.

**After the fixes (11.7–11.10), re-run against the baseline**
- A first re-run `20261017T133817` read 39/39, but the simulator still recorded inside the
  project, so a task that only reads could `cat` the state instead of calling tools. Moving the
  records out of the model's reach (`{mcp}`, e4caa31) made the next run honest.
- Clean re-run `20261017T135018` (deepseek-v4.1-flash, 39 runs, 0 ended on an error, $0.0137):
  against the baseline `20261017T131132`, turns 6.0 vs 7.1 and calls 9.8 vs 11.7 per run, cost
  350 vs 386 µ$. Done 36/39, against the baseline's 34/39 — but the baseline lost 4 runs to API
  502s, so on completion the fixes are about level; what moved is the path to the answer, a turn
  and two calls shorter. The prompt naming the servers meant no run invented what "MCP" is, and
  the written-call reminder fired 0 times (none wrote its calls as text).
- The three misses are the weak model, not crowbot: one answered a prototype as text and made no
  write call; one made the right lookups then declined the task instead of the rename; one
  searched the tracker many ways but never applied the label. Each is a model giving up at the
  last write, not a tool it got wrong.

## M12 — Empirical grounding (run it / script it / verify it) (planned 2026-10-17)
Plan: on compute/search tasks a model does the arithmetic in its head and never writes a helper
script; even a strong model grounds only by running the real program when one is at hand, and has
no fallback when it isn't (seen testing the IDA MCP on Windows and in this session's crackme runs).
The analyzer even penalises scripting as a smell. So: a prompt principle to run/script/verify
instead of computing in the head, the analyzer reclassified to reward it, compute/keygen battery
tasks, and a measured before/after (deepseek-v4.1-flash, a crow-2 reference) the M11 way.
- [x] 12.1 the system prompt: when an answer turns on a computation, a search or multi-step logic,
      make it checkable (run it, script it, brute-force it, debug it), not in the head
- [x] 12.2 the prompt says each bash runs in the project dir and shell state does not persist, so
      no re-`cd` and multi-step shell work goes in one command
- [x] 12.3 a solve-for-input op for the simulator: a new `Act::Check` evaluates a data-defined
      arithmetic predicate over an argument and accepts or rejects it
- [x] 12.4 compute/search + keygen tasks in the battery (sum over pages, a search, a pairing code
      from a published formula), neutrally themed
- [x] 12.5 the report rewards grounding: a signal and candidate for runs that answered without
      running or scripting a check; the python/node/heredoc smells dropped
- [x] 12.6 a measured before/after on the mcp suite, plus a crow-2 reference
- [x] 12.7 the findings to the user, and the Windows enablers to check (python on PATH, IDA's
      debug path)

**Runs (2026-10-17, local only in bench/runs/; ~$0.20 total, within the mcp cap)**
- Pilot `20261017T160854` (new prompt, mcp-pairing-code): the model scripted a modular inverse,
  got 46642, `verify` rejected it, re-read the spec, brute-forced 0..100000 in a script, found
  31337, `verify` accepted. The script plus the oracle caught an answer the inverse got wrong.
- Before `20261017T161022` (old prompt, deepseek-v4.1-flash, four compute/keygen tasks, 12 runs,
  all done, $0.0035): 3 of 12 grounded.
- After `20261017T161229` (new prompt, same tasks, 12 runs, all done, $0.0033): 10 of 12 grounded.
  Against the before, per run: grounded 0.8 vs 0.2 (+0.6), bash calls 0.8 vs 0.2, done 1.0 vs 1.0,
  cost 272 vs 292 µ$.
- Reference `20261017T161522` (crow-2, new prompt, four tasks, all done, $0.19): 2 of 4 grounded —
  it scripts the big sum and the keygen but does the two small sums in its head.

**Learned**
- The prompt line reliably moves a weak model to grounding on compute tasks: 3 → 10 of 12 runs,
  with no done-rate regression and slightly lower cost. What changed is the method, not the score.
- On these tasks done was already 1.0: the sums are small enough that deepseek adds them in its
  head correctly, so the prompt changed how it answered, not whether. The payoff shows where
  mental math fails — the keygen, where even a scripted inverse was wrong and the `verify` oracle
  saved it. A harder sum would turn the method gain into a score gain too.
- The strong model is selective: it scripts the big or hard computation and does the trivial one by
  hand — the conditional behaviour the prompt intends, not "always script".
- No ungrounded-miss candidate fired, because nothing missed; the candidate is for a run that
  missed having run or scripted nothing.

## M13 — Default model GLM 5.3 Flash, and model/effort remembered (2026-10-17)
- [x] 13.1 the default model is `glm-5.3-flash-crow` (data/defaults.toml); the bundled model
      snapshot (data/models.json) refreshed from the live list so the default resolves offline
- [x] 13.2 the model and effort a session chose are remembered in ~/.crowbot/prefs.json and apply
      next session, above the config files and below a one-off --model/--effort flag
      (src/config/prefs.rs, layered in settings::load; written from the TUI's picker and /effort)

## M14 — Fast reversing for weak models (planned 2026-10-18)
Plan: measure where a weak model's reverse-engineering wall-time goes, then cut it without dropping
the solve rate. Probes found wall-time is round-trips (one model call per step; tools in a reply
already run concurrently), caching is already tuned, and the battery records wall_ms. $10 cap; free
speedups only (round-trips + a sharper RE recipe, no effort/quality trade-offs).
- [x] 14.1 a reproducible harder-RE benchmark: a stripped `vault` ELF (three staged checks, a
      per-index transform), reversed with bash; a `crackme` fixture repo and a `re` suite
      (deepseek-v4.1-flash); only the binary ships, its source stays in bench/benchmarks/
- [x] 14.2 baseline the weak model(s) and build a timeline of where the seconds go (round-trips,
      tool-calls-per-reply, wasted turns), ranking the free-speedup levers
- [x] 14.3 cut round-trips: tried, no measured win, reverted —: sharpen the guidance so independent reads/searches go out in one reply
- [x] 14.4 RE workflow recipe: not needed — M12 grounding already makes weak models follow it; no unproven prompt additions
- [x] 14.5 findings reported; the `re` suite kept as a timed regression guard

**Baseline (2026-10-18, bench/runs/, within the $10 cap)**
- vault pilot `20261018T093526` and vault2 pilot `20261018T093828`: deepseek solved each in 6-7
  turns / ~26s — orient, strings, objdump, a python keygen, verify. The chained vault2 was no
  slower; the model reads the arrays from .rodata and inverts them.
- deepseek `20261018T094003` (6/6 done): mean wall ~31s, turns 6.5, calls/reply ~1.5.
- glm-5.3-flash-crow `20261018T094205` (6/6 done): mean wall 57.7s, turns 6.8, calls/reply 1.28,
  but wall ranged 31-130s. The 130s run had only 7 turns, so its time was per-turn latency
  (generation + upstream queue), not extra round-trips.

**Learned**
- Both weak models already reverse these reliably and fast (6-8 round-trips, always solved); the
  M12 grounding did the heavy lifting (they script and verify at once, no flailing). There is no
  pool of slow or failed runs to optimize against.
- The one free, controllable lever is batching: glm-flash is inconsistent (some runs one call per
  reply, 0 batched), and the recon phase (file -> strings -> objdump) often serializes across turns
  when it could be one reply. Per-turn latency (the 130s outlier) is off the table under
  "free speedups only".

**14.3 batching nudge — measured, reverted**
- Sharpened the Explore bullet to name the round-trip cost and cover bash inspections. Measured on
  glm-5.3-flash-crow (n=6 each): before wall 57.7s / turns 6.8 / calls-per-reply 1.28; after wall
  45.8s / turns 7.2 / calls-per-reply 1.15. Turns rose and batching fell — no win; the wall drop was
  just the absence of a latency outlier. Reverted (no prompt change without proof, as M11/M12).

**Learned (M14 so far)**
- Within "free speedups only", crowbot is already near-optimal for weak-model RE on these crackmes:
  both models sit at the round-trip floor (6-8 turns: orient, disassemble, script the inversion,
  verify) and always solve. The grounding from M12 is what makes that happen.
- The remaining wall-time is per-turn latency (model generation + upstream queue), not round-trips —
  the 131s glm run had only 7 turns. Cutting that means lowering reasoning effort or picking a
  faster model, both quality trade-offs the user ruled out. So the honest next decision is the
  user's: keep "free only" (benchmark stands as a regression guard, little more to win) or open the
  door to effort/model trade-offs ("speed first"), or raise difficulty (a gdb-only/obfuscated rung)
  to expose waste where weak models actually struggle.

**Overfitting check — three third-party crackmes (crackmes.one, local only, not committed)**
To be sure we were not overfitting to the vault style, ran deepseek-v4.1-flash (the cheapest model)
on three downloaded Linux x86-64 crackmes of rising difficulty:
- lvl1 `wheredakey` (stack-strcat + strcmp, overlapping-immediate writes): solved, 9 turns / 47s —
  iterated candidates against the binary to resolve the byte overlap.
- lvl3 `crackme` (UPX-packed, needs GLIBC_2.38 > the host's 2.35): solved, 16 turns / 84s — it
  recognised UPX, downloaded the upx tool and unpacked, patched the ELF's scanf version record to
  run on the older glibc, then read the flag by running it. The plaintext .rodata string was a
  decoy it saw through; the real flag was a quote.
- lvl5 `reapermaze` (self-decrypting code: a decoy FNV hash, then an xorshift-keystream-decrypted
  blob mmap'd RWX and called): solved, 8 turns / 31s — reproduced the xorshift in python, decrypted
  the blob statically, read the key.

**Learned (M14 complete)**
- No overfitting: 5/5 solved across vault, vault2 and the three downloads (strcmp, packing+ABI
  patching, self-modifying code), each in 30-90s, on the cheapest weak model.
- The weak model is strikingly resourceful: it downloads a missing tool (upx) and patches a binary
  for ABI compatibility on its own. M12 grounding generalises far past "run the binary".
- The only inefficiency seen is a `cd <abspath>` prefix on nearly every bash call (lvl3, 19/21),
  which the M12.2 note did not prevent — but it rides inside single bash calls, so it costs
  characters, not round-trips. Not a wall-time lever.
- Conclusion: within "free speedups only", crowbot is already a fast, reliable RE CLI for weak
  models; the remaining wall-time is per-turn latency (generation + upstream queue). The vault/vault2
  `re` suite stays as the committed timed regression guard.

## M15 — Batched calls shown grouped, and orient before speculating (2026-10-18)
- [x] 15.1 the TUI stacks a reply's tool calls with no blank between them (a batch reads as one
      group), keeping the blank above the first; `Block::Tool { first }` set from the `MessageEnd`
      boundary, the blank skipped for continuation calls in `Feed::measure`, mirrored in replay
- [x] 15.2 the prompt: start a task by orienting (fetch the link, read the named files, check the
      working dir and `git status`) instead of theorizing; ask when the request is unclear or needs
      info only the user has

## M16 — Fewer turns: pre-orient, persist cwd, best-tool note, batching (2026-10-18)
Plan: cut turns/tool-calls. Sessions showed the model burns opening turns discovering the
environment (the <env> block gives only cwd/OS/date/model) and re-`cd`s the full path on every bash
call (27/27 in the keygenme). $10 cap; measured before/after on the keygenme + re suite, solve rate
held.
- [x] 16.1 pre-orient: the first turn is handed a top-level listing + git branch/status/recent
      commits, as a `Part::Reminder` on the first prompt (once per conversation, top-level runs),
      so the model starts from facts (src/agent/orient.rs, data/prompts/orient.md, run.rs)
- [x] 16.2 persist bash cwd: a `cd` carries to the next call (Shared.cwd + a guarded leading-`cd`
      heuristic in bash), killing the cd-spam; the "don't re-cd" line dropped
- [x] 16.3 recommended-tools note: a short, evidence-based prompt section (kuna decompiler per
      decbench.com, requests/httpx over urllib, ripgrep, pwndbg, …) + a full reference doc
- [x] 16.4 revisit batching: re-measured; no clear one-call-per-reply pattern to target (code tasks are near-minimal), so no nudge added — M14 showed those do not land anyway
      remains
- [x] 16.5 measured before/after on a code recon task and the keygenme; kept all three

**Measured (2026-10-18, local; ~$0.3 total)**
Built pre-M16 (M15) and M16 binaries and ran them back-to-back to control for API-latency drift.
- Code recon task ("summarize the project + its git state"), M15 → M16:
  - crow-2 (strong, N=3): turns 3.3 → 3.0, calls 4.3 → 3.7, recon 3.3 → 2.7 — it trusts the
    handed-over listing/git and skips recon.
  - deepseek-v4.1-flash (weak, N=5): turns 3.8 → 3.4, calls 5.2 → 5.0, recon 4.4 → 4.6 — flat; it
    re-does its own recon regardless.
  - cd-prefix ≈ 0 for both: on a normal repo task the model does not `cd`.
- Keygenme (N=1 each): M15 36 turns, M16 45; both solved correctly. Variance is enormous (earlier
  runs were 20/36/45 turns), so this is noise. There the model `cd`-prefixed 44/53 calls under M16 —
  the cd-spam is a work-in-a-subdir artifact, and the model re-`cd`s by habit even with cwd
  persisting.

**Learned**
- A (pre-orient) helps the *stronger* model (and the default glm-flash is strong) by letting it skip
  recon; the weakest model is flat (sticky habit of re-looking). Kept.
- B (persist cwd) shows no measured turn/call win — cd-spam is rare (keygenme-only) and behavioural
  where it occurs — but cwd now persists, which is correct, so kept as hygiene.
- C (tool note) is tool-choice guidance, not a turn lever; unmeasured but low-risk; kept.
- Consistent with M14: a weak model's tool-use habits (recon, cd, one-call-per-reply) are sticky;
  harness nudges move the turn count only for stronger models. The model is the floor.

## M17 — Raise the floor: model benchmark, structural tools, grounding (2026-10-18)
The M16 finding ("the model is the floor") sets the direction: before more harness nudges a weak
model ignores, find the best floor and give the model structural capabilities it can't fumble.
Three strands: a model/effort benchmark, structural tools (webfetch, a decompiler), and stronger
grounding (auto-verify). In progress.

- [x] 17.1 model benchmark: every account model on the re suite, 3 repeats, default effort, timed.
- [x] 17.2 fix surfaced by the benchmark: a stream chunk with a null array field (gemini-3.8-flash
      sends `"tool_calls": null`) no longer drops the run (src/api/wire.rs, `null_as_default`).
- [x] 17.3 webfetch: no change needed — already browser-impersonating. The M16 research read
      src/io/http.rs (the API client, UA `crowbot/<ver>`) and missed src/io/fetch.rs, which uses
      `cffetch::CfClient` (full TLS fingerprint + per-profile cookie jars + a firewall signal).
- [x] 17.4 decompile tool: a `decompile` tool wrapping kuna, PATH-or-install (name a function to
      decompile it, or omit it to list the binary's functions; without kuna it returns the install
      path, objdump the fallback). Validated end-to-end against the stripped `vault` fixture.
- [x] 17.5 extend grounding (auto-verify): **built, tested, measured.** Mechanism as designed
      (configured-or-skip, off by default, trust-gated; settings.verify, src/agent/verify.rs, run.rs
      ~181). Measured verify-on vs -off on the code subset — no solve-rate gain, ~2x wall time (see
      Measured below). Kept as an opt-in; not recommended on by default.

### 17.5 design — auto-verify (configured-or-skip)
Catch "declared done but broken": when a top-level run that changed files would end, run the
project's configured check; a failure is handed back so the model fixes it. Harness-enforced, not a
prompt nudge a weak model ignores.
- **Command source (decided): configured-or-skip.** A new `verify` setting (crowbot config /
  AGENTS.md). Set → run it; unset → skip entirely, never guess. **Off by default, so shipping the
  mechanism changes nothing until a project opts in** (zero-risk ship; the opt-in benefit is measured
  before recommending it on).
- **Trust: free.** `verify` is not in trust.toml `untrusted_keys` (only model/effort are), so an
  untrusted repo's `verify` is dropped by `trust::admit` — only the user's own or a trusted folder's
  config can set the command crowbot auto-runs. No malicious-repo auto-exec.
- **Integration (no new machinery):** run.rs ~181, where `drain_follow` decides end-vs-continue.
  When the run would be `Done`, call `verify::failure(...)`; on a failure it returns a follow-up
  (the existing `Part::Reminder`/`deliver` path) and the run continues; on pass/none it ends.
- **Gate:** top-level (`parent.is_none()`), changed files (`run.before.get().is_some()`, set in
  batch.rs:88), a `verify` command set, under a retry cap. Only a clean nonzero exit counts as
  failure (timeout/signal/cancel → skip, so an inconclusive check never loops).
- **New data/limits:** `settings.verify: Option<String>`; `agent.verify_attempts` (cap 2),
  `agent.verify_timeout_secs` (300); `model_text.verify_failed`. Shell reuse via a `pub use
  bash::shell` re-export (write-it-once). New file `src/agent/verify.rs`.
- **Scope honesty:** a code-task lever (tests/lint after edits); RE already has M12 "run it" and a
  task-specific success check, so auto-verify won't change RE much.
- **Measurement:** code suite with `verify` ON (per-repo check from bench/repos.toml) vs OFF
  (have it: deepseek 14/16; the 2 failures, ufo-fix-join, are the target case). Win = solve rate up,
  turns not blown up, no loops. Needs battery per-task `verify` config injection (a small follow-up).

**Measured (2026-10-18, deepseek-v4.1-flash, 8-task code subset, 2 repeats, BATTERY_VERIFY=1)**
- Solve: 13/16 verify-on vs 14/16 off — the one difference is a flaky 2-turn early-stop
  (mi-fix-rstrip #1, exit 1, no verify involvement), so effectively flat / within noise.
- ufo-fix-join (the target "declared done but broken" case): still 0/2, turns up 10/14 -> 14/19 —
  verify fired (the fix cycles ran) but the model could not fix it even with the failure fed back,
  so it hit the cap and ended failed.
- Wall time ~2x (39s -> 75s avg): verify runs the project's whole test suite each time (mi-add-nth-
  last reached ~180s).
- **Verdict:** no solve-rate gain on this corpus, real wall-time cost. Consistent with "the model is
  the floor": auto-verify only helps when the model declares done with a failure it *can* fix given
  feedback — ~empty here (the model either already passed or could not fix regardless). Shipped as a
  correct, safe opt-in (off by default); not recommended on by default. It would help a stronger
  model that makes a careless-but-fixable slip, or a project whose check catches a class the model
  routinely forgets.

**Code comparison (2026-10-18, 8-task subset, 2 repeats, ~$0.22)** — does deepseek hold on code?
| model | solve | turns | wall | $/run |
| --- | --- | --- | --- | --- |
| deepseek-v4.1-flash | 14/16 | 5.9 | 39s | 0.0006 |
| glm-5.3-flash-crow *(current default)* | 14/16 | 7.0 | 79s | 0.0131 |
Both failed the *same* task (ufo-fix-join, 2/2); on every task both solved, deepseek used
equal-or-fewer turns. deepseek wins on code too: same solve rate, fewer turns, 2x faster, ~20x
cheaper. **Default flipped to deepseek-v4.1-flash** (data/defaults.toml) on this evidence; the user
chose the flip over keeping the UNCENSORED glm default (deepseek refused none of the RE tasks, and
the model stays user-overridable). The fake server's models fixture gained deepseek; the json
goldens moved to deepseek pricing.

**Finding (prompt note → wrong install):** a benchmark model, told to "use kuna", ran
`uv/pip install kuna` and got the unrelated PyPI package, not the decompiler (a GitHub-release
binary). 17.4's tool and note now point at the tool and the right source.

**Measured (2026-10-18, re suite, 3 repeats/model, ~$2.93 total, under the $10 cap)**
Turns is the metric (solved runs); solve = the task's own passcode check.

| model | solve | turns | wall | $/run | out $/1M |
| --- | --- | --- | --- | --- | --- |
| deepseek-v4.1-flash | 6/6 | 6.5 | 26s | 0.0007 | 0.12 |
| kimi-k3 | 6/6 | 6.5 | 53s | 0.0048 | 1.35 |
| mimo-2.6-flash-crow | 6/6 | 7.0 | 26s | 0.0464 | 1.50 |
| gpt-6.1-sol | 6/6 | 7.7 | 40s | 0.0047 | 0.90 |
| glm-5.3-flash-crow *(current default)* | 6/6 | 9.2 | 99s | 0.0137 | 0.60 |
| crow-2 *(old default)* | 6/6 | 10.5 | 32s | 0.1281 | 5.00 |
| gemini-3.8-flash *(after 17.2 fix)* | 4/6 | 7.5 | 39s | 0.0047 | 0.34 |
| qwen-3.8-crow | 4/6 | 16.8 | 184s | 0.2701 | 1.00 |
| gemini-4-argon | 3/6 | 6.7 | 60s | 0.0148 | 1.70 |

**Learned**
- deepseek-v4.1-flash wins the re suite outright: 100% solve, fewest turns (tied with kimi-k3),
  cheapest by ~20x against the current default, and fastest. The cheapest model is also the most
  turn-efficient at reversing — it reinforces M14's "deepseek carries RE".
- The current default glm-5.3-flash-crow is mid-pack on RE (9.2 turns, 99s); crow-2 (the old
  default) solves but takes more turns than the cheap models and costs ~180x deepseek.
- gemini-3.8-flash was not weak, it was broken (17.2): 0/6 all `bad_stream`, now 4/6.
- gemini-4-argon (3/6) and qwen-3.8-crow (4/6) genuinely underperform (they finish `done` with a
  wrong answer, or run long) — not a harness fault.
- deepseek held on the code suite too (above), so the global default was flipped to it. Still open
  as possible follow-ups: an effort sweep on the top few models may move turns further, and the code
  battery suite still names glm-flash (could track the default). Neither is blocking.

## M18 — Feed hierarchy, card details, visible command input (2026-10-19)
The transcript read flat with gaps. Three fixes, built as planned.
- [x] 18.1 card details: a `[decompile]` section (binary + function + pseudocode head); a title
      fallback so any unknown-arg tool (every MCP tool) shows its first scalar arg values instead of
      a bare name (an explicit `title = []` stays bare). data/tool_cards.toml, src/frontend/tool_card.rs.
- [x] 18.2 visible command input: a multi-line bash command (a heredoc or `python3 -c -` script) now
      shows full, capped, as a muted preamble above the output — it was off the one-line header.
      src/frontend/tool_card.rs `input()`, src/frontend/tui/calls.rs.
- [x] 18.3 collapsible subagent tree: a `task`'s child calls, once discarded, are captured as a tree
      and flattened into indented child cards, collapsed to the count by default, expanded like
      thinking (click / Ctrl-T); nested subagents indent per depth. `Block::Tool.depth`, a
      buffer-then-flatten-on-task-end so parallel subagents never interleave. src/frontend/tui/feed/*.
- [x] 18.4 batch rail: a reply's ≥2 adjacent calls share a `│ ` left rail (a lone call none); the
      draw cache keys on (width, Decor) so a card re-renders when a sibling joins its batch.
- [x] 18.5 replay reconstructs the tree from each task result's linked child session
      (details.session), through a loader in conversation.rs so the feed stays UI-only; a missing
      child leaves the task flat. An empty/undetailed summary now shows no line, not `?: 0`.

**Learned**
- Two card systems: `Check::preview` feeds only the permission prompt; the transcript card is built
  from the call's arguments JSON + tool_cards.toml, which is why decompile/MCP showed nothing and the
  fix was data, not the preview.
- Keeping subagent children as flat sibling entries with a `depth` field (not a parent-owned subtree)
  kept the width cache, the row→block click/copy map, and `measure` simple; collapse is a depth
  "floor" in measure. The one real correctness risk — interleaved parallel-subagent children — is
  handled by buffering on the running task and flattening only at its end.

## M19 — Fewer turns: repeated-call guard + guarded finish (2026-10-20)
From the three-pass study of recorded weak-model sessions (waste is ~90% execution/harness). Two
string-free fixes, chosen with the user.
- [x] 19.1 repeated-call guard: `doom_loop` now counts interleaved identical (name, args) calls
      across the turn (not just a consecutive run), gated on the output not changing — fulfils its
      own documented CEILING. Catches the #1 flailing signature (stale-cache re-decompile marathons).
- [x] 19.2 guarded `finish` tool: ends a run after a batch only if every other call in it succeeded;
      routes through the same end path as a no-tool reply, so M17.5 auto-verify composes. Prompt
      nudges the model to batch it with its last action.

**Measured (deepseek, mcp re + mutation tasks, 2 repeats, ~$0.02)**
- **A works as designed.** The guard fired on `mcp-re-fix-prototype` (the stale-cache marathon); its
  after-runs were 9/9 turns with no 15–20 turn marathons (before: 15, 17, 20 among 12 runs, mean
  10.5). It catches genuine futile loops with no downside — but they are the rare extreme, so the
  aggregate effect is a safety-net, not a broad turn cut. Correctly did NOT fire on diverse-query
  flailing (a 21-turn tracker run of distinct searches — different args, different results).
- **B is adopted but neutral on turns.** `finish` was called in 3/8 sessions — but every time
  **alone, as its own final turn** (db-fix: update, verify, then finish-alone), never batched with
  the action. So it *replaced* the free-text "Done" turn with a finish-call turn: same round-trip,
  **zero turns saved** (db-fix stayed at 4). The weak model adopts the tool yet keeps its
  done-in-a-fresh-turn habit.

**Learned**
- The reconciliation of "the model is the floor": a harness fix that needs **no model behavior
  change** works (A — the guard fires on futile loops on its own); one that needs a **behavior
  change** (B — optimistically batch `finish` with the action) hits the same sticky-habit wall as
  batching, pre-orient and auto-verify did. The confirmation turn is structurally irreducible without
  the model signalling "done" *inside* the action turn, which weak models won't do.
- Both kept (safe): A is a no-downside safety-net against extreme flailing; B is adopted, composes
  with auto-verify, and is a foundation a stronger model would batch — but neutral on turns for weak
  models today. Not recommended as a turn lever until a model batches it.

## M20 — Presets: the model picked per request by rules (2026-10-24)
Two outside PRs (#2 roles, #4 subagent models, agent files, claims) each added a UI layer so
subagents could run on cheaper models. Chosen instead, with the user: a preset, picked in `/models`
like a model, whose ordered rules pick each request's model and effort from the mode, the agent
(main, subagent, a kind), the context sent, and the plan's allowance or the balance. No new card;
claims dropped (two subagents at most, and they need a weak model to cooperate). From #4, only its
fix for a subagent ending with `finish` (cherry-picked).
- [x] 20.0 cherry-pick #4's `finish` summary fix
- [x] 20.1 `src/models/preset.rs`: the file, its checks (a catch-all last, no dead rules, known
      modes, agents, efforts), `resolve` against the catalog, `pick`, `create` from the template
- [x] 20.2 `src/models/choice.rs`: a model or a preset by one name, everywhere a model id goes
- [x] 20.3 `RunCtx::picked` per request; the picked model's window decides a summary; subagents
      pick by kind; prompts record the preset, so resume keeps it
- [x] 20.4 the account in `Shared`; a headless run asks once, only for a preset that reads it
- [x] 20.5 `/models` lists presets under a heading, `+ new preset`; `/presets [new <name>]`,
      `crowbot presets`; the bar shows `preset → model`; `/status` names the file
- [x] 20.6 README, scenarios (`expect.models`), pty tests

**Learned**
- The wire layer already took a conversation of mixed models (a reasoning trace is replayed only to
  the model that wrote it; cost is per reply), because `/models` could always switch mid-session.
  What assumed one model per run was the window check and what the frontends showed.
- Rows the selection steps over (a heading, a rule) were the one new picker idea; and a list longer
  than its window has to say so, or the presets under nine models are never seen.
- Open: a stale `model` in prefs.json (a model crowbot dropped, a preset deleted) still fails the
  start rather than giving way to the config's, as a resumed session's does.
