#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
workflow="$repo_root/.github/workflows/release.yml"
for marker in \
  'RELEASE_NOTES_PATH="docs/releases/$RELEASE_TAG.md"' \
  '--notes-file "$RELEASE_NOTES_PATH"' \
  '--draft' \
  '--draft=false' \
  '--latest'; do
  if ! grep -Fq -- "$marker" "$workflow"; then
    echo "release privacy contract: release workflow is missing versioned-notes marker: $marker" >&2
    exit 1
  fi
done

publish_count="$(grep -Fc -- '--draft=false' "$workflow")"
if [[ "$publish_count" != 1 ]]; then
  echo "release privacy contract: expected exactly one public transition, found $publish_count" >&2
  exit 1
fi
checksum_line="$(grep -Fn -- 'sha256sum -c SHA256SUMS' "$workflow" | cut -d: -f1)"
upload_line="$(grep -Fn -- 'gh release upload "$RELEASE_TAG" dist/*' "$workflow" | cut -d: -f1)"
publish_line="$(grep -Fn -- '--draft=false' "$workflow" | cut -d: -f1)"
if (( publish_line <= checksum_line || publish_line <= upload_line )); then
  echo "release privacy contract: public transition must follow checksum verification and asset upload" >&2
  exit 1
fi
sandbox="$(mktemp -d "${TMPDIR:-/tmp}/ashide-release-privacy-contract.XXXXXX")"
trap 'rm -rf "$sandbox"' EXIT
fixture_repo="$sandbox/repo"
tokens_file="$sandbox/private-tokens"
mkdir -p "$fixture_repo/script"
cp "$repo_root/script/check_release_privacy" "$fixture_repo/script/check_release_privacy"
chmod +x "$fixture_repo/script/check_release_privacy"

(
  cd "$fixture_repo"
  git init -q
  git config user.email test@example.invalid
  git config user.name "Release Privacy Contract"
  printf '%s\n' 'ssh:ssh-config:remote-fixture-primary' 'ssh-config:remote-fixture-primary' 'root@remote-fixture-primary' > fixture.txt
  git add fixture.txt script/check_release_privacy
  git commit -qm init
)
printf '%s\n' 'private-target-987654' > "$tokens_file"

run_checker() {
  ASHIDE_RELEASE_PRIVACY_ROOT="$fixture_repo" \
  ASHIDE_PRIVATE_RELEASE_TOKENS_FILE="$tokens_file" \
    "$fixture_repo/script/check_release_privacy" "$@" >/dev/null 2>&1
}

run_checker

# Binary string adjacency can synthesize an apparent config prefix plus enum
# variant even though no publishable text owns that fixture. Generic fixture
# validation is text-structural; the private denylist still scans binary bytes.
printf '\0%s%s\0' 'ssh-config:' 'Loaded' > "$fixture_repo/generated.bin"
(
  cd "$fixture_repo"
  git add generated.bin
)
run_checker
(
  cd "$fixture_repo"
  git rm -q -f generated.bin
)

printf '%s\n' 'private-target-987654' > "$fixture_repo/private.txt"
(
  cd "$fixture_repo"
  git add private.txt
)
if run_checker; then
  echo "release privacy contract: local private token probe unexpectedly passed" >&2
  exit 1
fi
(
  cd "$fixture_repo"
  git rm -q -f private.txt
)

printf 'ssh-config:%s\n' 'customer-production-host' > "$fixture_repo/fixture.txt"
(
  cd "$fixture_repo"
  git add fixture.txt
)
if run_checker; then
  echo "release privacy contract: non-generic bare ssh-config fixture probe unexpectedly passed" >&2
  exit 1
fi


printf 'root@%s\n' 'customer-production-host' > "$fixture_repo/fixture.txt"
(
  cd "$fixture_repo"
  git add fixture.txt
)
if run_checker; then
  echo "release privacy contract: non-generic root SSH target probe unexpectedly passed" >&2
  exit 1
fi

(
  cd "$fixture_repo"
  git checkout -q -- fixture.txt
)
artifact_source="$sandbox/artifact-source"
artifact_root="$sandbox/artifacts"
mkdir -p "$artifact_source" "$artifact_root"
printf 'ssh-config:%s\n' 'customer-production-host' > "$artifact_source/leak.txt"
tar -czf "$artifact_root/payload.tar.gz" -C "$artifact_source" leak.txt
if run_checker --artifacts "$artifact_root"; then
  echo "release privacy contract: archived text fixture probe unexpectedly passed" >&2
  exit 1
fi

printf '%s\n' 'release privacy contract passed'
