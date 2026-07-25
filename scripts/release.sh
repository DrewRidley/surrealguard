#!/usr/bin/env bash
# Release helper for SurrealGuard.
#
#   scripts/release.sh check          # verify only — safe, changes nothing
#   scripts/release.sh bump 0.4.0     # rewrite versions, then re-run `check`
#   scripts/release.sh publish        # the real thing (prompts once, then irreversible)
#
# Publishing is irreversible: a crates.io version can be yanked but never
# replaced, and npm is the same. `check` and `bump` never publish.

set -euo pipefail
cd "$(dirname "$0")/.."

# Dependency order — a crate must be on the registry before anything that
# depends on it can build there. surrealguard-wasm is `publish = false`.
CRATES=(
  surrealguard-tree-sitter-surrealql
  surrealguard-syntax
  surrealguard-embed
  surrealguard-diagnostics
  surrealguard-workspace
  surrealguard-codegen
  surrealguard-macros
  surrealguard-rs
  surrealguard-lsp
  surrealguard
)

crate_dir() {
  for d in crates/*/; do
    [ "$(grep -m1 '^name' "$d/Cargo.toml" | cut -d'"' -f2)" = "$1" ] && { echo "$d"; return; }
  done
}

cmd_check() {
  echo "== workspace version =="
  grep -m1 '^version' Cargo.toml

  echo "== build =="
  cargo build --release 2>&1 | grep -E '^(error|warning: unused)|Finished' | tail -3

  echo "== tests =="
  cargo test 2>&1 | grep -E 'test result: (ok|FAILED)' \
    | awk '{s+=$4; f+=$6} END {print "   passed:", s, " failed:", f; if (f>0) exit 1}'

  echo "== oracle (all-valid corpus: the count must not grow) =="
  local ws=/Users/drewridley/Documents/Projects/workshop/database
  if [ -d "$ws" ]; then
    ( cd "$ws" && "$OLDPWD/target/release/surrealguard" check --json 2>/dev/null \
      | python3 -c "import sys,json,collections
d=json.load(sys.stdin); c=collections.Counter(x['code'] for x in d['diagnostics'])
print('   total:', sum(c.values()), dict(c))" )
  else
    echo "   (corpus not present — skipped)"
  fi

  echo "== packaging dry-run (catches missing files / bad metadata) =="
  for c in "${CRATES[@]}"; do
    printf '   %-38s ' "$c"
    if cargo package -p "$c" --allow-dirty --quiet >/dev/null 2>&1; then echo ok
    else echo "FAILED — run: cargo package -p $c"; fi
  done
}

cmd_bump() {
  local v="${1:?usage: release.sh bump <version>}"
  echo "bumping workspace -> $v"
  # Only the [workspace.package] version; crates inherit via version.workspace.
  perl -0pi -e "s/(\[workspace\.package\][^\[]*?\nversion = )\"[^\"]+\"/\${1}\"$v\"/s" Cargo.toml
  grep -m1 -A1 '\[workspace.package\]' Cargo.toml | grep version

  # npm packages that are published (private/example packages are skipped).
  for p in $(find packages -name package.json -not -path '*/node_modules/*' 2>/dev/null); do
    python3 - "$p" "$v" <<'PY'
import json, sys
path, ver = sys.argv[1], sys.argv[2]
d = json.load(open(path))
if d.get("private") or not d.get("name", "").startswith("@surrealguard/"):
    raise SystemExit
d["version"] = ver
for field in ("dependencies", "devDependencies", "peerDependencies"):
    for dep in d.get(field, {}):
        if dep.startswith("@surrealguard/"):
            d[field][dep] = ver
json.dump(d, open(path, "w"), indent=2)
open(path, "a").write("\n")
print(f"   {d['name']} -> {ver}")
PY
  done
  echo "now re-run: scripts/release.sh check"
}

cmd_publish() {
  echo "About to publish to crates.io and npm. This CANNOT be undone."
  read -r -p "Type the version to confirm: " confirm
  local v; v=$(grep -m1 -A2 '\[workspace.package\]' Cargo.toml | grep -m1 '^version' | cut -d'"' -f2)
  [ "$confirm" = "$v" ] || { echo "aborted (expected $v)"; exit 1; }

  for c in "${CRATES[@]}"; do
    echo "== publishing $c =="
    cargo publish -p "$c" || { echo "FAILED at $c — fix, then resume from here"; exit 1; }
    # crates.io needs a moment to index before a dependent can resolve it.
    sleep 20
  done

  echo "== npm =="
  pnpm -r publish --access public --no-git-checks
}

case "${1:-check}" in
  check)   cmd_check ;;
  bump)    cmd_bump "${2:-}" ;;
  publish) cmd_publish ;;
  *) echo "usage: $0 {check|bump <version>|publish}"; exit 1 ;;
esac
