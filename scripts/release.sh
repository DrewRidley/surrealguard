#!/usr/bin/env bash
# Release helper for SurrealQL Analyzer.
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
# depends on it can build there. surrealql-analyzer-wasm is `publish = false`.
CRATES=(
  surrealql-analyzer-tree-sitter-surrealql
  surrealql-analyzer-syntax
  surrealql-analyzer-embed
  surrealql-analyzer-diagnostics
  surrealql-analyzer-workspace
  surrealql-analyzer-codegen
  surrealql-analyzer-macros
  surrealql-analyzer-rs
  surrealql-analyzer-lsp
  surrealql-analyzer
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

  # Not a count — a triage gate. The corpus contains genuinely broken SurrealQL,
  # so findings may legitimately grow; what must hold is that every finding is
  # accounted for, and that none silently STOPPED firing (a count hides that:
  # one gained plus one lost looks like no change at all).
  echo "== oracle (triage gate) =="
  python3 scripts/oracle.py check || {
    echo "   triage the findings above, then: scripts/oracle.py update"
    exit 1
  }

  # `--no-verify` on purpose: verification builds the tarball against the
  # *registry*, so any crate using an API added in this same release fails until
  # its dependency is published. That is expected in a coordinated release and
  # resolves itself as `publish` walks the dependency order. What is worth
  # checking ahead of time is metadata and file inclusion, which this does.
  echo "== packaging dry-run (metadata + file inclusion) =="
  local pending=0
  for c in "${CRATES[@]}"; do
    printf '   %-38s ' "$c"
    local out
    out=$(cargo package -p "$c" --allow-dirty --no-verify --quiet 2>&1) && { echo ok; continue; }
    # A sibling at the new version isn't on the registry yet — unavoidable in a
    # coordinated bump, and it resolves as `publish` walks dependency order.
    # Only a failure with some OTHER cause is a real problem.
    if grep -q "candidate versions found which didn't match" <<<"$out"; then
      echo "pending (a sibling crate isn't published at this version yet)"
      pending=$((pending + 1))
    else
      echo "FAILED"
      sed 's/^/       /' <<<"$out" | head -4
    fi
  done
  if [ "$pending" -gt 0 ]; then
    echo "   note: $pending crate(s) pending. Expected after a version bump — the"
    echo "         registry has no sibling at this version until publish runs."
    echo "         To validate packaging properly, run \`check\` BEFORE bumping."
  fi
}

cmd_bump() {
  local v="${1:?usage: release.sh bump <version>}"
  echo "bumping workspace -> $v"
  # The [workspace.package] version; crates inherit it via version.workspace.
  perl -0pi -e "s/(\[workspace\.package\][^\[]*?\nversion = )\"[^\"]+\"/\${1}\"$v\"/s" Cargo.toml
  grep -m1 -A1 '\[workspace.package\]' Cargo.toml | grep version

  # Intra-workspace dependencies pin a version alongside their path (cargo
  # requires it to publish). Those are NOT covered by version.workspace and must
  # move in lockstep, or the workspace stops resolving the moment it is bumped.
  echo "   intra-workspace dependency pins:"
  local count=0
  for manifest in Cargo.toml crates/*/Cargo.toml; do
    local n
    n=$(perl -0pi -e "BEGIN{\$c=0} \$c += s/((?:^|\n)\s*(?:[a-z-]+ *= *\{[^}]*?)?path = \"[^\"]*(?:crates\/)?[^\"]*\"[^}]*?version = )\"[^\"]+\"/\${1}\"$v\"/g; END{print STDERR \$c}" "$manifest" 2>&1 >/dev/null || true)
    if [ -n "$n" ] && [ "$n" != "0" ]; then
      printf '     %-34s %s pin(s)\n' "$manifest" "$n"
      count=$((count + n))
    fi
  done
  echo "     ($count total)"
  if grep -rqE 'path = "[^"]*", version = "(?!'"${v//./\\.}"')' Cargo.toml crates/*/Cargo.toml 2>/dev/null; then
    echo "   WARNING: some path dependencies still pin an older version"
  fi

  # npm packages that are published (private/example packages are skipped).

  # Every publishable npm package. NOTE the search covers more than packages/:
  # the `surrealql-analyzer` CLI shim lives in npm/ and was silently skipped when this
  # only looked at packages/ and only matched the @surrealdb/analyzer- scope — so
  # `npx surrealql-analyzer` kept installing an old version.
  for p in $(find packages npm -name package.json -not -path '*/node_modules/*' 2>/dev/null); do
    python3 - "$p" "$v" <<'PY'
import json, sys
path, ver = sys.argv[1], sys.argv[2]
d = json.load(open(path))
name = d.get("name", "")
if d.get("private") or not (name == "surrealql-analyzer" or name.startswith("@surrealdb/analyzer-")):
    raise SystemExit
d["version"] = ver
for field in ("dependencies", "devDependencies", "peerDependencies"):
    for dep in d.get(field, {}):
        if dep.startswith("@surrealdb/analyzer-"):
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

  # The `surrealql-analyzer` npm shim and the Zed extension both download a prebuilt
  # binary from the GitHub Release for their version. Publishing either before
  # that release exists gives users a 404 instead of a stale version — strictly
  # worse. The `v*` tag is what triggers .github/workflows/release.yml to build
  # and upload those assets, so it must land first.
  # A dirty tree is not a warning here. `cargo publish` reads the working
  # directory, not the tag, so uncommitted changes go to crates.io permanently
  # while the tag points at something else. This used to surface as a failure
  # partway through the crate loop, after earlier crates had already published.
  if [ -n "$(git status --porcelain)" ]; then
    echo "Working tree is dirty. cargo publish reads the tree, not the tag —"
    echo "commit first, or you publish something no commit records:"
    git status --short | sed 's/^/    /'
    exit 1
  fi

  if ! git rev-parse "v$v" >/dev/null 2>&1; then
    echo
    echo "No tag v$v yet. The GitHub Release assets (CLI + LSP, 5 targets) are"
    echo "built by pushing it:"
    echo
    echo "    git tag v$v && git push origin v$v"
    echo
    echo "Wait for .github/workflows/release.yml to finish, confirm the assets at"
    echo "    https://github.com/surrealdb/analyzer/releases/tag/v$v"
    echo "then re-run this. crates.io does not depend on the tag; npm does."
    read -r -p "Publish crates.io now and do npm later? [y/N] " go
    [ "$go" = "y" ] || exit 1
    local skip_npm=1
  # Existing is not enough — it must name THIS commit. Tagging before committing
  # the bump produces a v0.5.0 whose Cargo.toml still says 0.4.1, so the release
  # workflow builds binaries that report the previous version. That happened, and
  # nothing caught it: the tag was present, so the check passed.
  elif [ "$(git rev-parse "v$v")" != "$(git rev-parse HEAD)" ]; then
    echo
    echo "Tag v$v exists but points at a different commit:"
    printf '    v%-8s -> %s  (version %s)\n' "$v" \
      "$(git rev-parse --short "v$v")" \
      "$(git show "v$v:Cargo.toml" 2>/dev/null | grep -m1 -A2 '\[workspace.package\]' | grep -m1 '^version' | cut -d'"' -f2)"
    printf '    %-9s -> %s  (version %s)\n' HEAD "$(git rev-parse --short HEAD)" "$v"
    echo
    echo "The release assets were built from the tag, so they do not match what"
    echo "you are about to publish. Move it:"
    echo
    echo "    git tag -f v$v && git push origin v$v --force"
    echo
    echo "then wait for the run to go green on all five targets and re-run this."
    exit 1
  fi

  for c in "${CRATES[@]}"; do
    echo "== publishing $c =="
    cargo publish -p "$c" || { echo "FAILED at $c — fix, then resume from here"; exit 1; }
    # crates.io needs a moment to index before a dependent can resolve it.
    sleep 20
  done

  if [ "${skip_npm:-0}" = "1" ]; then
    echo "== npm skipped — tag v$v first, then: pnpm -r publish --access public =="
    exit 0
  fi
  # `pnpm -r` covers whatever pnpm-workspace.yaml lists. The CLI shim lives in
  # npm/, which was missing from that list — so this used to publish 4 of the 5
  # packages and silently leave `npx surrealql-analyzer` on an old version.
  echo "== npm =="
  pnpm -r publish --access public --no-git-checks
  echo "   published:"
  pnpm -r list --depth -1 2>/dev/null | grep -E '^(surrealql-analyzer|@surrealdb/analyzer-)' | sed 's/^/     /' 
}

case "${1:-check}" in
  check)   cmd_check ;;
  bump)    cmd_bump "${2:-}" ;;
  publish) cmd_publish ;;
  *) echo "usage: $0 {check|bump <version>|publish}"; exit 1 ;;
esac
