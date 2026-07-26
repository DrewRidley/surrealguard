#!/usr/bin/env bash
# Reproducible build of the SurrealGuard analyzer for the browser playground.
#
# Produces target/wasm32-wasip1/release/surrealguard_wasm.wasm and copies it
# into web/public/playground/.
#
# Toolchain (macOS / Homebrew):
#   rustup target add wasm32-wasip1
#   brew install wasi-libc wasi-runtimes llvm
#
# The tree-sitter C parser has no libc on bare wasm; we point the `cc` crate at
# Homebrew's LLVM clang with the wasi-libc sysroot so its build finds one.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

# --- locate the wasi toolchain ------------------------------------------------
LLVM_PREFIX="${LLVM_PREFIX:-$(brew --prefix llvm 2>/dev/null || echo /opt/homebrew/opt/llvm)}"
WASI_SYSROOT="${WASI_SYSROOT:-$(brew --prefix wasi-libc 2>/dev/null || echo /opt/homebrew/opt/wasi-libc)/share/wasi-sysroot}"
CLANG="$LLVM_PREFIX/bin/clang"
LLVM_AR="$LLVM_PREFIX/bin/llvm-ar"

if [[ ! -x "$CLANG" ]]; then
  echo "error: clang not found at $CLANG (brew install llvm)" >&2
  exit 1
fi
if [[ ! -d "$WASI_SYSROOT" ]]; then
  echo "error: wasi sysroot not found at $WASI_SYSROOT (brew install wasi-libc)" >&2
  exit 1
fi

# --- point the `cc` crate at the wasi toolchain -------------------------------
export CC_wasm32_wasip1="$CLANG"
export CFLAGS_wasm32_wasip1="--target=wasm32-wasip1 --sysroot=$WASI_SYSROOT"
export AR_wasm32_wasip1="$LLVM_AR"

echo "Building surrealguard-wasm for wasm32-wasip1 (release)…"
cargo build -p surrealguard-wasm --target wasm32-wasip1 --release

# Honour CARGO_TARGET_DIR so the build can be pointed at scratch space
# (`CARGO_TARGET_DIR=/tmp/web-target crates/wasm/build-wasm.sh`) without the
# copy below silently looking in the wrong place.
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
ARTIFACT="$TARGET_DIR/wasm32-wasip1/release/surrealguard_wasm.wasm"
DEST="web/public/playground/surrealguard_wasm.wasm"
cp "$ARTIFACT" "$DEST"
echo "Wrote $DEST ($(du -h "$DEST" | cut -f1))"
echo "Proof harness: node web/public/playground/harness.mjs"
