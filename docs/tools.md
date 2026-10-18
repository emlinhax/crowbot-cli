# Recommended tools (reference)

Best-in-class external tools for an autonomous coding / reverse-engineering CLI on Linux. The short
prompt note in `data/prompts/system.md` carries only the few highest-leverage "the default is wrong"
picks; this is the fuller catalogue, kept out of the prompt so it does not bloat every turn.
Researched 2026-10-18 (decompiler rankings from decbench.com's own scoreboard). Everything is
free/open-source unless marked **paid**.

## The high-leverage picks (also in the prompt)
- **HTTP from code → `httpx` or `requests`, not `urllib`.** urllib's default `User-Agent:
  Python-urllib/3.x` is hard-blocked (403) by Cloudflare/WAFs. `httpx` adds async + HTTP/2.
- **Protected site → `curl_cffi` (`impersonate="chrome"`) or Playwright, not plain `requests`/`curl`.**
  Modern anti-bot fingerprints the TLS/JA3 handshake; a spoofed header is not enough. Playwright when
  there's a JS/Turnstile challenge.
- **Reverse engineering → read a decompiler's pseudocode, not raw disassembly.** `kuna` is the top
  free decompiler on decbench.com. Drive gdb with `pwndbg`/`gef`, not bare.
- **Search → `rg` (ripgrep) over `grep -r`, `fd` over `find`, `ast-grep` for code structure.**
- **Data → `jq` for JSON; `duckdb` SQL for large/tabular CSV/JSON/Parquet.**

## Decompilers (ranked by decbench.com, snapshot 2026-10-09)
Union % = share of functions perfect on ≥1 of structural / type / recompile metrics.
- **Unoptimised (O0):** kuna 51.4, Hex-Rays/IDA 49.1 (**paid**), angr 47.5, Binary Ninja 37.9
  (**paid**), Ghidra 37.5.
- **Optimised (O2):** kuna 37.3 ≈ Hex-Rays 37.0 ≈ angr 36.9 > Ghidra 31.5. (LLM-agent entries top the
  hard sets but cost ~$6/function and minutes each — only for a few critical functions.)
- **Speed:** kuna ≈ Hex-Rays (~0.11 s/function), far ahead of Ghidra/Binary Ninja.
- **Pick:** `kuna` (Apache-2.0, single Rust binary, `kuna decompile ./bin main`, `kuna install-skill`;
  https://kuna.noelo.org) — a prebuilt binary from its GitHub releases (github.com/Noelo-Lab/kuna),
  **not** the unrelated `kuna` on PyPI. crowbot's `decompile` tool wraps it. `Ghidra` is the best
  free full *suite* (GUI + headless scripting). `angr` doubles as symbolic execution. IDA/Hex-Rays
  and Binary Ninja are paid.

## Disassemblers / RE frameworks
Ghidra (free suite), IDA Pro+Hex-Rays (**paid**, industry standard), Binary Ninja (**paid**, great
API), rizin+Cutter (free, scriptable; Cutter bundles the Ghidra decompiler), angr (symbolic
execution). Baseline already present: `objdump` / `nm` / `readelf`.

## Debuggers
`gdb` + **pwndbg** (auto context, heap/ROP tooling; also drives LLDB) or **gef** (lighter, GDB-only).
`lldb` for newer clang toolchains. **Frida** for dynamic instrumentation of a live process.

## Binary unpacking / packer ID
`upx -d` for UPX. **unblob** and **binwalk v3** for recursive firmware/container extraction. **Detect
It Easy** (`diec`) to identify a packer/compiler. (VMProtect/Themida resist these — manual dynamic
unpacking.)

## Emulation
`qemu-user` (run foreign-arch binaries), **Unicorn** (emulate code snippets), **Qiling** (full binary
emulation with loaders/syscalls).

## HTTP clients
Python: `httpx` / `requests`; `curl_cffi` for TLS-fingerprint sites. Shell: `curl` (default), `xh` or
HTTPie for JSON APIs, `curl-impersonate` for browser TLS, `aria2c` for big parallel downloads.

## Web scraping / anti-bot
Ladder: `requests`/`curl` → `curl_cffi`/`curl-impersonate` (TLS fingerprint) → **Playwright**
(JS/Turnstile). HTML parsing: `selectolax`/`lxml` over BeautifulSoup; from shell `htmlq`/`pup`.

## Code search
`rg` (ripgrep), `fd`, `ast-grep` (structural, tree-sitter, 20+ langs), `comby` (structural rewrite).

## JSON / data
`jq` (+ `gron` to make JSON greppable), `yq` (YAML/XML/TOML), Miller `mlr` (CSV/TSV), **DuckDB** (SQL
over files, fast).

## UI / TUI libraries (maintained, 2026)
- **Rust:** `ratatui` (+ `crossterm`), `clap`, `indicatif`, `comfy-table`, `inquire`.
- **Python:** `rich`, `textual`, `typer`/`click`, `questionary`, `tqdm`.
- **Node:** `ink`, `chalk`, `commander`/`yargs`/`oclif`, `inquirer`, `ora`, `listr2`.

## Sources
decbench.com (+ `/data/aggregates.json`) · kuna.noelo.org, github.com/Noelo-Lab/kuna ·
github.com/BurntSushi/ripgrep · github.com/lexiforest/curl_cffi · github.com/pwndbg/pwndbg ·
ast-grep.github.io · duckdb.org · ratatui.rs
