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
- [ ] CI green on all three OSes (needs the GitHub remote)

**Learned**
- reqwest is 0.13: TLS features are `rustls` (aws-lc-rs provider, builds fine on MSVC without
  cmake) plus `rustls-platform-verifier` for OS roots. The ring/webpki plan item was moot.
- Static-CRT release build on Windows: 4.5 MB.
- Binary crates warn on unread `pub` fields, so data fields are added only when read.
- Windows Terminal via crossterm (no kitty enhancement): every key also sends a Release event
  (filter to Press/Repeat); Shift+Tab is `BackTab`+SHIFT; Shift+Enter keeps SHIFT; Ctrl+J is
  `Char('j')`+CONTROL. A paste is plain key presses (13 for "one
two
three", 2 of them Enter)
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
- [x] CI: `.github/actions/boringssl` on every job; glibc is the required Linux release target
- [ ] First CI run (needs the GitHub remote): proves the toolchain on all three OSes, and
      `release-musl` decides whether Linux ships fully static (it may fail until then)
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

## M4 — Account UX in the TUI
## M5 — Context + sessions (compaction, shadow-git /undo)
## M6 — Subagents + MCP
## M7 — Release (`prod` → GitHub Releases, installers)
- Added 2026-10-02, untried until the first `prod` push: every push to `prod` runs `ci.yml`
  through `release.yml` and publishes `crowbot-<target>` for musl Linux, Apple-silicon macOS and
  Windows, plus `SHA256SUMS`.
- Added 2026-10-03: the repository is public (MIT OR Apache-2.0, history rewritten to a noreply
  author and without cffetch's live-site probes); release builds carry their tag; `crowbot update`
  and a daily background update for interactive release builds (SHA256SUMS-checked, the program
  swapped where it stands); `crowbot install` / `uninstall`, and on Windows a double-clicked
  release offers to install itself (user PATH in the registry, Apps & Features entry).
- Open: signing (minisign for updates, Authenticode and notarisation for SmartScreen and
  Gatekeeper), Intel macOS and arm64 Linux targets, a website installer script.
