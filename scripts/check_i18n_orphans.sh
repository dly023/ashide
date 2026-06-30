#!/usr/bin/env bash
# Report Fluent keys in app/i18n/en/warp.ftl that have no t!/flt! reference in Rust.
# Also lists keys that appear as string literals elsewhere (possible dynamic t_or use).
set -euo pipefail
cd "$(dirname "$0")/.."

python3 <<'PY'
import re
import subprocess
from pathlib import Path

ftl = Path("app/i18n/en/warp.ftl").read_text()
keys = set(re.findall(r'^([a-zA-Z0-9_-]+)\s*=', ftl, re.M))
refs = set()
for path in Path("app").rglob("*.rs"):
    text = path.read_text(errors="ignore")
    for m in re.finditer(r'(?:crate::)?t!\(\s*"([^"]+)"', text):
        refs.add(m.group(1))
    for m in re.finditer(r'flt!\(\s*"([^"]+)"', text):
        refs.add(m.group(1))

orphans = sorted(keys - refs)
print(f"en/warp.ftl keys: {len(keys)}")
print(f"direct Rust t!/flt! refs: {len(refs & keys)}")
print(f"orphan keys (no direct t!/flt! ref): {len(orphans)}")

dynamic = []
for key in orphans:
    r = subprocess.run(
        ["rg", "-l", key, "app", "crates", "--glob", "!**/i18n/**"],
        capture_output=True,
        text=True,
    )
    if r.returncode == 0:
        dynamic.append(key)

print(f"orphans with non-i18n string hits (keep): {len(dynamic)}")
print(f"orphans with zero non-i18n hits: {len(orphans) - len(dynamic)}")
PY
