# surrealql-analyzer-wasm

WebAssembly bindings that expose the SurrealQL Analyzer engine to the browser
playground. The crate wraps `surrealql-analyzer-workspace` and compiles to
`wasm32-wasip1`, exporting a tiny C ABI so any WASI shim (browser or Node) can
call `analyze(schemaText, queryText)` and receive a JSON array of diagnostics.

## Why wasm32-wasip1 (and not bare wasm32-unknown-unknown)

The analyzer depends on the `tree-sitter-surrealql` C parser
(`parser.c` + `scanner.c`), which `#include`s `<stdlib.h>`, `<string.h>`,
`<stdio.h>`, etc. Those headers do not exist on `wasm32-unknown-unknown` (no
libc), so the `cc`-driven build fails with `'stdlib.h' file not found`.

`wasm32-wasip1` ships a libc (wasi-libc). We point the `cc` crate at Homebrew's
LLVM clang with the wasi-libc sysroot, and the C parser compiles cleanly. All
the pure-Rust dependencies (`surrealdb-types`, `geo`, `chrono`, `regex`,
`serde_json`, …) already build for `wasm32-wasip1` unchanged.

## Toolchain (macOS / Homebrew)

```sh
rustup target add wasm32-wasip1
brew install llvm wasi-libc wasi-runtimes
```

- `llvm` — clang capable of emitting wasm objects.
- `wasi-libc` — the sysroot: headers + `libc.a` under
  `$(brew --prefix wasi-libc)/share/wasi-sysroot`.
- `wasi-runtimes` — compiler-rt / libc++ for wasi (present for completeness;
  the pure-C parser does not need libc++).

No full `wasi-sdk` download is required — Homebrew's `llvm` + `wasi-libc`
supply everything.

## Build

```sh
./crates/wasm/build-wasm.sh
```

The script exports the `cc`-crate overrides and runs the cargo build:

```sh
export CC_wasm32_wasip1="$(brew --prefix llvm)/bin/clang"
export CFLAGS_wasm32_wasip1="--target=wasm32-wasip1 --sysroot=$(brew --prefix wasi-libc)/share/wasi-sysroot"
export AR_wasm32_wasip1="$(brew --prefix llvm)/bin/llvm-ar"
cargo build -p surrealql-analyzer-wasm --target wasm32-wasip1 --release
```

Output: `target/wasm32-wasip1/release/surrealql_analyzer_wasm.wasm` (~4 MB), copied to
`web/public/playground/surrealql_analyzer_wasm.wasm`.

## Proof (headless)

```sh
node web/public/playground/harness.mjs
```

Analyzes `SELECT ssn FROM user` against
`DEFINE TABLE user SCHEMAFULL; DEFINE FIELD name ON user TYPE string;` and
asserts an `E1002` unknown-field diagnostic. Exit code 0 on success.

## Browser playground

`web/public/playground/index.html` fetches the `.wasm`, instantiates it with a
~40-line self-contained WASI shim (`sg-analyzer.mjs`, no external runtime), and
runs live diagnostics as you type. Serve `web/public` over HTTP and open
`/playground/`.

## ABI

- `sg_alloc(len) -> ptr` — reserve guest memory for a host-written string.
- `sg_dealloc(ptr, len)` — free a buffer from `sg_alloc` or `sg_analyze`.
- `sg_analyze(schema_ptr, schema_len, query_ptr, query_len) -> u64` — returns
  `(result_ptr << 32) | result_len`; the buffer holds UTF-8 JSON, owned by the
  caller.
- `sg_analyze2(...) -> u64` — same signature and same ownership, richer JSON.

`sg_analyze`'s JSON is an array of `{ code, severity, message, start, end }`,
one per diagnostic on the query, with byte offsets relative to the query text.

`sg_analyze2`'s JSON is `{ diagnostics, statements }` — the same diagnostics,
plus one `{ kind, start, end, response }` per top-level query statement.
`response` is the statement's inferred response kind, rendered the way an
editor would show a value's type at a position, or `null` when the statement
does not respond with one. It is a **second export rather than a wider
`sg_analyze`** because the `.wasm` and the `.mjs` that drives it are separate
files with separate cache lifetimes: a page can hold a script newer than its
module, so hosts feature-detect (`typeof exports.sg_analyze2 === "function"`)
and fall back to diagnostics-only rather than breaking.
