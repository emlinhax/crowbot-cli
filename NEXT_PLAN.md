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
- [ ] 6.4 a child's cost counts in the parent's, live and after resume
- [ ] 6.5 the task card, headless notes, `child` events in `--json`
- [ ] 6.6 parallel children within `subagent.parallel`
- [ ] 6.7 `general`: the parent's tools; its edits are covered by /undo
- [ ] 6.8 a plain No inside a child stops the parent
- [ ] 6.9 README and Learned (subagents)
- [ ] 6.10 trust lists a left-out table entry by entry, secrets by name only
- [ ] 6.11 `[mcp.<name>]` servers start with the session (`io::proc` child, JSON-RPC, handshake);
      /status names them
- [ ] 6.12 their tools join the registry (`<server>__<tool>`, optional `tools` allowlist) and ask
      as `mcp` / `mcp_write`
- [ ] 6.13 read-only tools (`readOnlyHint`) run in PLAN by the rules and take no snapshot
- [ ] 6.14 interrupts send `notifications/cancelled`; a server that stops is reported once
- [ ] 6.15 README and Learned (MCP)
- e2e: one local batch after 6.5, 6.8, 6.12 and 6.14; each batch is pushed so CI covers Windows.
## M7 — Release (`prod` → GitHub Releases, installers)
- Added 2026-10-02: every push to `prod` runs `ci.yml` through `release.yml` and publishes
  `crowbot-<target>` for musl Linux, Apple-silicon macOS and Windows, plus `SHA256SUMS`. Proven
  2026-10-06 by `v0.2.0-2` through `v0.2.0-4`.
- Added 2026-10-03: the repository is public (MIT OR Apache-2.0, history rewritten to a noreply
  author and without cffetch's live-site probes); release builds carry their tag; `crowbot update`
  and a daily background update for interactive release builds (SHA256SUMS-checked, the program
  swapped where it stands); `crowbot install` / `uninstall`, and on Windows a double-clicked
  release offers to install itself (user PATH in the registry, Apps & Features entry).
- Open: signing (minisign for updates, Authenticode and notarisation for SmartScreen and
  Gatekeeper), Intel macOS and arm64 Linux targets, a website installer script.

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
