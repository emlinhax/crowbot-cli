# Next plan

Phases from the approved design. Every sub-step ends with `make lint test` (plus `make e2e` from M1).
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
- [ ] 3.7 docs + manual Windows Terminal checklist

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
## M4 — Account UX in the TUI
## M5 — Context + sessions (compaction, shadow-git /undo)
## M6 — Subagents + MCP
## M7 — Release (`prod` → GitHub Releases, installers)
