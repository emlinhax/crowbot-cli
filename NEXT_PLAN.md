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
- [ ] `crowbot keytest` on Windows Terminal: Shift+Tab, Shift+Enter, multi-line paste, key release
- [ ] CI green on all three OSes (needs the GitHub remote)

**Learned**
- reqwest is 0.13: TLS features are `rustls` (aws-lc-rs provider, builds fine on MSVC without
  cmake) plus `rustls-platform-verifier` for OS roots. The ring/webpki plan item was moot.
- Static-CRT release build on Windows: 4.5 MB.
- Binary crates warn on unread `pub` fields, so data fields are added only when read.

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
## M3 — TUI core
## M4 — Account UX in the TUI
## M5 — Context + sessions (compaction, shadow-git /undo)
## M6 — Subagents + MCP
## M7 — Release (`prod` → GitHub Releases, installers)
