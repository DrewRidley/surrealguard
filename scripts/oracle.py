#!/usr/bin/env python3
"""Triage gate for the real-world corpus.

The corpus is NOT all-valid — it contains genuinely broken SurrealQL. So holding
the finding *count* flat is the wrong invariant, and an actively harmful one: it
pressures a change toward suppressing a correct new diagnostic rather than
recording it. That is not hypothetical. Gating aggregate promotion on a GROUP
clause was declined purely because it would add +2 findings here, even though the
engine rejects both of those queries outright:

    SELECT VALUE math::sum(size_bytes) FROM file WHERE ...
    -> Incorrect arguments for function math::sum(). Expected `array<number>`
       but found `10`

The invariant that actually matters: **every finding is accounted for**. New
findings are fine — they must be triaged and recorded with a verdict. A finding
that DISAPPEARS is equally interesting, because a check that stopped firing is
usually a regression, and a bare count hides that entirely (one gained + one lost
looks identical to no change).

  scripts/oracle.py check    compare the corpus against the baseline
  scripts/oracle.py update   rewrite the baseline, preserving existing verdicts
"""

import json
import os
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
BASELINE = REPO / "tests" / "oracle_baseline.txt"
CORPUS = Path("/Users/drewridley/Documents/Projects/workshop/database")
# The release build, unless a caller points elsewhere: an agent or a bisect run
# builds into its own CARGO_TARGET_DIR precisely so it does not disturb the
# binary the editor is using.
BIN = Path(os.environ.get("SG_ORACLE_BIN") or REPO / "target" / "release" / "surrealguard")

HEADER = """\
# Oracle baseline — every finding the real-world corpus produces, with a verdict.
#
# The corpus is NOT all-valid; it contains genuinely broken SurrealQL. A growing
# count is therefore not a failure. An UNTRIAGED finding is, and so is one that
# silently disappears.
#
# Verdicts:
#   genuine  — the corpus SurrealQL really is wrong (say how it was verified)
#   expected — correct behaviour on input we accept as-is
#   BUG      — a false positive. Any line with this verdict is a TODO on us.
#
# Regenerate with `scripts/oracle.py update` (existing verdicts are preserved).
"""


def findings():
    """Every finding the corpus produces, as (code, file, message)."""
    if not CORPUS.is_dir():
        sys.exit(f"corpus not found at {CORPUS}")
    # `check` exits non-zero whenever any finding is error-severity, which this
    # corpus has by design — the exit code is not a failure signal here.
    out = subprocess.run(
        [str(BIN), "check", "--json"], cwd=CORPUS, capture_output=True, text=True
    ).stdout
    got = []
    for d in json.loads(out)["diagnostics"]:
        src = d.get("source", "").replace("file://", "").replace(f"{CORPUS}/", "")
        got.append((d["code"], src, d["message"]))
    return sorted(got)


def parse_baseline():
    """Recorded findings -> verdict. Missing file yields an empty mapping."""
    if not BASELINE.exists():
        return {}
    verdicts, key = {}, None
    for line in BASELINE.read_text().splitlines():
        if line.startswith("#") or not line.strip():
            continue
        if m := re.match(r"^(\S+)\s+(\S*)$", line):
            key = (m.group(1), m.group(2))
        elif line.strip().startswith("verdict:") and key:
            verdicts[key] = line.split("verdict:", 1)[1].strip()
    return verdicts


def render(got, verdicts):
    lines = [HEADER, f"# total: {len(got)} finding(s)\n"]
    for code, src, msg in got:
        verdict = verdicts.get((code, src), "TRIAGE ME — genuine / expected / BUG?")
        lines.append(f"{code}  {src}\n    {msg}\n    verdict: {verdict}\n")
    return "\n".join(lines)


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else "check"
    got = findings()
    verdicts = parse_baseline()

    if cmd == "update":
        BASELINE.parent.mkdir(exist_ok=True)
        BASELINE.write_text(render(got, verdicts))
        untriaged = sum(1 for c, s, _ in got if (c, s) not in verdicts)
        print(f"wrote {BASELINE.relative_to(REPO)} — {len(got)} finding(s)")
        if untriaged:
            print(f"  {untriaged} need a verdict — search for 'TRIAGE ME'")
        return

    recorded = set(verdicts)
    seen = {(c, s) for c, s, _ in got}
    added = [f for f in got if (f[0], f[1]) not in recorded]
    removed = sorted(recorded - seen)

    for code, src, msg in added:
        print(f"  NEW      {code}  {src}\n           {msg}")
    for code, src in removed:
        print(f"  GONE     {code}  {src}")
        print("           a check stopped firing here — regression, or an intended fix?")
    if bugs := [k for k, v in verdicts.items() if v.startswith("BUG")]:
        print(f"  {len(bugs)} known false positive(s) still recorded")

    if added or removed:
        print(f"\n  {len(added)} new, {len(removed)} gone. Triage, then: scripts/oracle.py update")
        sys.exit(1)
    print(f"  all {len(got)} finding(s) accounted for")


if __name__ == "__main__":
    main()
