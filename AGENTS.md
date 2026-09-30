# How code is written here

Read this before changing anything. Every codebase starts out hard-wired to its first case: two of something, a fixed pair of names, a check that demands exactly what existed on day one. Opening it up to the third case then costs a day of edits across every layer. Build so that the next case is an addition, not a rewrite.

**Data, not code.** Numbers, shapes, catalogs, prompts and lookup tables live in data files (TOML, JSON, Markdown) and are read by generic code. A new system's configuration is a new data file with its values, not constants inside a function.

**Many solutions, one shape.** Where several implementations answer one need, they are same-shaped files in one folder behind one interface, swapped by name: providers that each expose the same entry point, backends with identical signatures, plugins registered in one index. Adding one is adding a file; nothing else learns its name.

**Every file contains itself.** One concern per file, named for it. Nothing "and also this" tucked into an unrelated file; a thing goes where its concern lives. A file that starts collecting unrelated helpers gets split.

**Write it once.** Code needed twice is one shared implementation with a mode or a parameter, never a copy. Globals and settings live in one place and are read from there, never redeclared where they are used.

**Wrap the outside world.** Network, files, processes, the terminal and the clock go through one module each, so a change of transport, a retry or a cache is one edit. No inline HTTP calls, file reads or shell-outs in feature code; a request made in two places is one function.

**Shapes that extend.** Anything that can have more than one is a list, iterated by index or id, even while it has one or two today. A record names every party it concerns, never "the other one". A limit is data with a stated reason, checked in one place, not implied by a loop bound or a fixed-size table. A file format carries lists and optional fields, so a new field is an addition and old files still load.

**Still lazy.** None of this is a licence to build what nobody asked for: no abstraction with one implementation, no config for a value that never changes, no modules for later. Generalise the shape and the placement, not the behaviour. When a shortcut has a ceiling, leave a comment tagged `CEILING:` naming the ceiling and the upgrade path. That is the one tag this project uses; `grep -rn CEILING:` lists every known shortcut.

**Comments say why, and say it briefly.** Code says what it does; names and structure carry that. A comment is for the reason a reader couldn't infer from the code (a constraint, a trade-off, a ceiling) and it is a line or two, not a paragraph. No banners, no restating the line beneath, no narrating obvious steps, no commented-out code. A block that needs a long comment to be understood needs to be rewritten instead.

**Work in verified steps.** Big work is a phased plan with sub-steps, each run against the test suite (and the benchmark, when behaviour must not move) before the next. Golden outputs are regenerated only for a deliberate change, with the proof stated in the commit.

**e2e runs in CI.** `make lint` and `make test` before every push. The e2e suite runs on every
push to `main` and again in the deploy, so run it locally only for a critical change, one that
could break a feature (a flow, billing, auth, an API), and then once per batch, never repeatedly.

**Work on `main`.** Commit straight to it; no feature branches. The only other branch is `prod`,
which is the deploy trigger and is merged into only on an explicit go. A branch that is not one of
those two is a leftover.

The rest: `README.md` for the layout, the commands and the checks; `NEXT_PLAN.md` for the phases and what each one learned.

## Project notes

- e2e here means the real `crowbot` binary driven against an in-process fake crowbot server
  (`tests/e2e/`), never the live API. `make smoke` is the only thing that spends real money.
- Only `src/io/` touches the network, filesystem, processes, terminal or clock; `clippy.toml` bans
  the raw calls elsewhere.
- `reference/` (gitignored: local only, absent from a fresh clone) holds the opencode and pi clones
  plus crowbot's docs. It is read-only:
  port designs from it, never code paths into it.
