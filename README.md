# crowbot

A small terminal coding agent for [crowbot.sh](https://crowbot.sh). One backend, one binary.

Read `AGENTS.md` before changing anything; `NEXT_PLAN.md` tracks the phases.

## Commands

```sh
crowbot login                    # pair this machine (or: login --key <account number>)
crowbot signup                   # create an account: no email, no password, no recovery
crowbot -p "explain this repo"   # one prompt, reply on stdout (pipe input works too)
crowbot --json "..."             # the same, as one JSON event per line
crowbot models [--refresh]       # live models, prices and limits
crowbot help                     # every command (each is also /name inside a session)
```

`--model <id>`, `--effort low|medium|high|max` and `--mode manual|auto|plan` apply to any run.
MANUAL asks before edits, commands and fetches (a headless `-p` run declines those prompts);
AUTO allows everything; PLAN is read-only apart from its plan file. The key comes from
`CROWBOT_API_KEY`, else `~/.crowbot/auth.json` (DPAPI-sealed on Windows, 0600 elsewhere).

## Layout

| Path | What lives there |
|---|---|
| `data/` | Every number, prompt, catalog and command spec, embedded with `include_str!`. Change behaviour here first. |
| `data/limits.toml` | Every limit, each with its reason. |
| `data/endpoints.toml` | crowbot's origins and endpoints; `CROWBOT_API_URL` / `CROWBOT_CHAT_URL` override origins. |
| `data/commands/` | One spec per command (name, summary, usage). |
| `data/errors.toml` | Every error kind: title, hint, whether it is retried. |
| `data/prompts/` | System prompt pieces and mode reminders. |
| `data/modes/` | One file per permission mode (MANUAL, AUTO, PLAN): verdicts and locked rules. |
| `data/tools/` | Each tool's description (`.md`) and argument schema (`.schema.json`). |
| `data/model_text.toml` | Everything crowbot tells the model on the user's behalf (declines, errors). |
| `data/shells.toml` | Which shell runs commands, per OS, and its environment. |
| `data/command_arity.toml` | How "always allow" generalises a command (`git commit *`). |
| `src/io/` | The only code that touches network, files, terminal or clock (enforced by `clippy.toml`). |
| `src/api/` | crowbot endpoints, errors, SSE parsing, chat streaming, retry, pairing, signup. |
| `src/agent/` | The loop (`run.rs`), tool batches, context repair, doom-loop guard, shared run state. |
| `src/tools/` | One file per tool behind the `Tool` trait, registered in `tools/mod.rs`; `edit_match.rs` is the fuzzy matcher. |
| `src/permission/` | Rules (last match wins), the gate that layers them with the mode, shell splitting. |
| `src/mode.rs` | Loads `data/modes/`. |
| `src/session/` | Append-only JSONL session files under `~/.crowbot/sessions/<project>/`. |
| `src/frontend/` | Headless `-p` / `--json` output. |
| `src/commands/` | One file per command, registered in `commands/mod.rs`. |
| `src/text/` | Formatting shared by the CLI and (later) the TUI. |
| `src/{app,cli,settings,paths,limits}.rs` | Startup, flag parsing, layered settings, locations, limits. |
| `tests/e2e/` | The real binary against an in-process fake crowbot (`fake_crowbot.rs`). |
| `tests/fixtures/` | Data the fake serves (`sse/*.sse` streams, `api/*.json`), sample `projects/`. |
| `tests/fixtures/scenarios/` | One TOML per agent scenario (args, scripted replies, expected files and requests). |
| `tests/golden/edit/` | Edit-matcher cases: `before.txt` + `args.json` → `after.txt` or `error.txt`. |
| `tests/smoke.rs` | The only tests that touch the real API (`make smoke`). |
| `reference/` | Gitignored clones of opencode and pi plus crowbot's docs; designs are ported from here. |

Settings layer in this order, later wins: `data/defaults.toml`, `~/.crowbot/config.toml`,
`.crowbot/config.toml` in the project, then flags. `CROWBOT_HOME` moves `~/.crowbot`.

## Checks

GNU make, run from Git Bash on Windows (`winget install ezwinports.make`).

| Target | Runs | When |
|---|---|---|
| `make lint` | `cargo fmt --check`, `clippy -D warnings` (includes the io-wrapper rule) | before every push |
| `make test` | unit tests, data-integrity tests | before every push |
| `make e2e` | binary vs fake crowbot | CI on every push to `main`; locally only for critical changes |
| `make golden` | rewrite golden expectations | only for a deliberate change |
| `make smoke` | real API round trip, needs `CROWBOT_SMOKE_KEY` (costs a fraction of a cent) | by hand, never in CI |
| `make build` | static release binary | as needed |

## Conventions

- Comments say why, briefly. `CEILING:` marks a known shortcut and its upgrade path: `grep -rn CEILING: src`.
- Commit straight to `main`; `prod` is the release trigger and is merged into only on an explicit go.
