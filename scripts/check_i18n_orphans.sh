#!/usr/bin/env bash
# Fail closed when an English Fluent message has no statically provable owner.
set -euo pipefail
cd "$(dirname "$0")/.."

python3 <<'PY'
import re
import sys
from collections import Counter
from pathlib import Path

FTL_ID = re.compile(r'^([A-Za-z0-9_-]+)\s*=', re.M)
RUST_STRING = r'"([^"\\]*(?:\\.[^"\\]*)*)"'


def read(path: str) -> str:
    return Path(path).read_text(errors="replace")


def message_ids(path: str) -> list[str]:
    return FTL_ID.findall(read(path))


def require_source(path: str, pattern: str, description: str) -> str:
    text = read(path)
    if re.search(pattern, text, re.M | re.S) is None:
        raise SystemExit(f"{path}: missing i18n owner contract: {description}")
    return text


locale_paths = {
    "en": "app/i18n/en/warp.ftl",
    "zh-CN": "app/i18n/zh-CN/warp.ftl",
    "ja": "app/i18n/ja/warp.ftl",
}
locale_ids = {locale: message_ids(path) for locale, path in locale_paths.items()}
for locale, ids in locale_ids.items():
    duplicates = sorted(key for key, count in Counter(ids).items() if count > 1)
    if duplicates:
        raise SystemExit(f"{locale_paths[locale]}: duplicate Fluent IDs: {duplicates}")

keys = set(locale_ids["en"])
rust_sources = {
    path: path.read_text(errors="replace")
    for root in (Path("app"), Path("crates"))
    for path in root.rglob("*.rs")
}

categories: dict[str, set[str]] = {}

# Literal localization macros are compiler-visible owners.
macro_pattern = re.compile(
    rf'(?<![A-Za-z0-9_])(?:(?:crate::)?(?:t|t_static|flt)|t_with_fallback)!\(\s*{RUST_STRING}'
)
categories["literal_macros"] = {
    match.group(1)
    for text in rust_sources.values()
    for match in macro_pattern.finditer(text)
}

# Literal t_or calls are explicit owners too.
t_or_literal_pattern = re.compile(rf'(?<![A-Za-z0-9_])(?:(?:crate::)?i18n::)?t_or\(\s*{RUST_STRING}')
categories["literal_t_or"] = {
    match.group(1)
    for text in rust_sources.values()
    for match in t_or_literal_pattern.finditer(text)
}

# Onboarding owns dynamic keys through one installed localizer. Every call-site key
# must remain a literal so this inventory cannot silently accept generated IDs.
require_source(
    "app/src/lib.rs",
    r'onboarding::set_localizer\(\|key\|\s*crate::i18n::t_or\(key,\s*key\)\);',
    "onboarding localizer must route its key directly through t_or",
)
localized_pattern = re.compile(rf'\b(?:localized|localized_static)\(\s*{RUST_STRING}')
categories["onboarding_localized"] = {
    match.group(1)
    for text in rust_sources.values()
    for match in localized_pattern.finditer(text)
}

# Terminal input has an explicit key registry plus named *_KEY constants. The
# shared translator is the only dynamic consumer of that registry.
terminal_input = require_source(
    "app/src/terminal/input.rs",
    r'fn\s+translate_input_key\([^)]*\)\s*->\s*String\s*\{\s*crate::i18n::t_or\(key,\s*key\)\s*\}',
    "terminal key registry must use translate_input_key -> t_or",
)
terminal_constants = {
    match.group(1)
    for match in re.finditer(
        rf'(?ms)^\s*(?:pub(?:\([^)]*\))?\s+)?const\s+[A-Z0-9_]*KEY[A-Z0-9_]*\s*:\s*&str\s*=\s*{RUST_STRING}\s*;',
        terminal_input,
    )
}
hint_registry = re.search(
    r'(?ms)^const\s+AGENT_MODE_HINT_KEYS\s*:\s*&\[&str\]\s*=\s*&\[(.*?)\];',
    terminal_input,
)
if hint_registry is None:
    raise SystemExit("app/src/terminal/input.rs: AGENT_MODE_HINT_KEYS registry is missing")
terminal_constants.update(re.findall(RUST_STRING, hint_registry.group(1)))
require_source(
    "app/src/terminal/input.rs",
    r'translate_input_key\(key\)',
    "AGENT_MODE_HINT_KEYS selection must flow through translate_input_key",
)
require_source(
    "app/src/terminal/input.rs",
    r'fn\s+translate_cli_agent_rich_input_hint_key\([^)]*\).*?t_or\(\s*CLI_AGENT_RICH_INPUT_HINT_KEY,',
    "CLI agent rich-input key must flow through t_or",
)
categories["terminal_input_registry"] = {
    value for value in terminal_constants if value.startswith("terminal-")
}

# HOA cards deliberately store title/description IDs as data. Lock both the
# literal registry and its t_or consumption.
hoa = require_source(
    "app/src/workspace/hoa_onboarding/welcome_banner.rs",
    r'for\s+item\s+in\s+FEATURE_ITEMS\s*\{',
    "HOA feature registry must be iterated by the renderer",
)
for field in ("title_key", "description_key"):
    if re.search(rf't_or\(item\.{field},\s*item\.{field}\)', hoa) is None:
        raise SystemExit(
            "app/src/workspace/hoa_onboarding/welcome_banner.rs: "
            f"FEATURE_ITEMS.{field} must flow directly through t_or"
        )
categories["hoa_feature_registry"] = set(
    re.findall(rf'(?:title_key|description_key):\s*{RUST_STRING}', hoa)
)

# Non-literal t_or is allowed only at the explicitly modeled owner boundaries
# above, plus named constants whose values are already inventoried.
allowed_dynamic_t_or = {
    ("app/src/lib.rs", "key"),
    ("app/src/terminal/input.rs", "key"),
    ("app/src/workspace/hoa_onboarding/welcome_banner.rs", "item.title_key"),
    ("app/src/workspace/hoa_onboarding/welcome_banner.rs", "item.description_key"),
}
const_values: dict[str, str] = {}
for text in rust_sources.values():
    for match in re.finditer(
        rf'(?ms)^\s*(?:pub(?:\([^)]*\))?\s+)?const\s+([A-Z][A-Z0-9_]*)\s*:\s*&str\s*=\s*{RUST_STRING}\s*;',
        text,
    ):
        const_values[match.group(1)] = match.group(2)

unknown_dynamic_t_or: list[str] = []
t_or_call = re.compile(r'(?<![A-Za-z0-9_])(?:(?:crate::)?i18n::)?t_or\(\s*([^,\n]+)')
for path, text in rust_sources.items():
    if str(path) == "app/src/i18n.rs":
        continue
    for match in t_or_call.finditer(text):
        argument = match.group(1).strip()
        if argument.startswith('"'):
            continue
        if argument in const_values:
            categories.setdefault("named_t_or_constants", set()).add(const_values[argument])
            continue
        if (str(path), argument) in allowed_dynamic_t_or:
            continue
        line = text.count("\n", 0, match.start()) + 1
        unknown_dynamic_t_or.append(f"{path}:{line}: {argument}")
if unknown_dynamic_t_or:
    raise SystemExit(
        "unclassified non-literal t_or call sites; add an explicit owner inventory:\n"
        + "\n".join(unknown_dynamic_t_or)
    )

all_references = set().union(*categories.values())
dynamic_categories = {
    "literal_t_or",
    "onboarding_localized",
    "terminal_input_registry",
    "hoa_feature_registry",
    "named_t_or_constants",
}
missing_dynamic_messages = sorted(
    set().union(*(categories.get(name, set()) for name in dynamic_categories)) - keys
)
if missing_dynamic_messages:
    raise SystemExit(
        "Dynamic localization owners reference missing English Fluent IDs:\n"
        + "\n".join(missing_dynamic_messages)
    )

unowned = sorted(keys - all_references)
print(f"en/warp.ftl keys: {len(keys)}")
covered: set[str] = set()
for name, references in categories.items():
    owned = references & keys
    newly_owned = owned - covered
    covered.update(owned)
    print(f"{name}: {len(owned)} ({len(newly_owned)} newly owned)")
print(f"owned Fluent keys: {len(covered)}")
print(f"unowned Fluent keys: {len(unowned)}")

for locale in ("zh-CN", "ja"):
    locale_keys = set(locale_ids[locale])
    print(
        f"{locale}/warp.ftl keys: {len(locale_keys)} "
        f"(missing_vs_en={len(keys - locale_keys)}, extra_vs_en={len(locale_keys - keys)})"
    )

if unowned:
    print("Unowned Fluent IDs:", file=sys.stderr)
    print("\n".join(unowned), file=sys.stderr)
    raise SystemExit(1)
PY
