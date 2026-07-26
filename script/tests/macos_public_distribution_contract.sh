#!/usr/bin/env bash

set -euo pipefail

WORKSPACE_ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RESOLVER="$WORKSPACE_ROOT_DIR/script/macos/resolve_signing_mode"
VERIFIER="$WORKSPACE_ROOT_DIR/script/macos/verify_public_distribution"
TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/ashide-macos-public-contract.XXXXXX")"
trap 'rm -rf "$TEST_DIR"' EXIT

expect_failure() {
  if "$@" >/dev/null 2>&1; then
    echo "expected command to fail: $*" >&2
    exit 1
  fi
}

clean_env=(env -i PATH="$PATH" HOME="${HOME:-/tmp}")
signing_env=(
  ASHIDE_DEVELOPER_ID_CERT=fixture-cert
  ASHIDE_DEVELOPER_ID_CERT_PASSWORD=fixture-cert-password
  ASHIDE_CODESIGN_KEYCHAIN_PASSWORD=fixture-keychain-password
  ASHIDE_NOTARIZATION_APPLE_ID=fixture@example.invalid
  ASHIDE_NOTARIZATION_PASSWORD=fixture-app-password
  ASHIDE_NOTARIZATION_TEAM_ID=ABCDEFGHIJ
)

[[ "$("${clean_env[@]}" "$RESOLVER")" == "adhoc" ]]
expect_failure "${clean_env[@]}" "$RESOLVER" public
expect_failure "${clean_env[@]}" ASHIDE_DEVELOPER_ID_CERT=partial "$RESOLVER"
[[ "$("${clean_env[@]}" "${signing_env[@]}" "$RESOLVER")" == "signed" ]]
expect_failure "${clean_env[@]}" "${signing_env[@]:0:5}" ASHIDE_NOTARIZATION_TEAM_ID=invalid "$RESOLVER"

mkdir -p "$TEST_DIR/mock-bin" "$TEST_DIR/Ashide.app"
: > "$TEST_DIR/Ashide.dmg"

cat > "$TEST_DIR/mock-bin/codesign" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
if [[ " $* " == *" --display "* ]]; then
  case "${SIGNATURE_CASE:-valid}" in
    valid)
      cat >&2 <<'META'
Authority=Developer ID Application: Ashide Fixture (ABCDEFGHIJ)
TeamIdentifier=ABCDEFGHIJ
CodeDirectory v=20500 flags=0x10000(runtime)
META
      ;;
    adhoc)
      cat >&2 <<'META'
Signature=adhoc
TeamIdentifier=not set
CodeDirectory v=20500 flags=0x2(adhoc)
META
      ;;
    wrong_team)
      cat >&2 <<'META'
Authority=Developer ID Application: Ashide Fixture (ZZZZZZZZZZ)
TeamIdentifier=ZZZZZZZZZZ
CodeDirectory v=20500 flags=0x10000(runtime)
META
      ;;
    no_runtime)
      cat >&2 <<'META'
Authority=Developer ID Application: Ashide Fixture (ABCDEFGHIJ)
TeamIdentifier=ABCDEFGHIJ
CodeDirectory v=20500 flags=0x0(none)
META
      ;;
  esac
fi
MOCK

cat > "$TEST_DIR/mock-bin/xcrun" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
[[ "${STAPLER_FAIL:-0}" != "1" ]]
MOCK

cat > "$TEST_DIR/mock-bin/spctl" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
if [[ " $* " == *" --type execute "* ]]; then
  [[ "${APP_GATEKEEPER_FAIL:-0}" != "1" ]]
elif [[ " $* " == *" --type open "* ]]; then
  [[ "${DMG_GATEKEEPER_FAIL:-0}" != "1" ]]
else
  exit 2
fi
MOCK
chmod +x "$TEST_DIR/mock-bin/codesign" "$TEST_DIR/mock-bin/xcrun" "$TEST_DIR/mock-bin/spctl"

verifier_env=(env -i PATH="$TEST_DIR/mock-bin:$PATH" HOME="${HOME:-/tmp}")
"${verifier_env[@]}" "$VERIFIER" "$TEST_DIR/Ashide.app" "$TEST_DIR/Ashide.dmg" ABCDEFGHIJ >/dev/null
expect_failure "${verifier_env[@]}" SIGNATURE_CASE=adhoc "$VERIFIER" "$TEST_DIR/Ashide.app" "$TEST_DIR/Ashide.dmg" ABCDEFGHIJ
expect_failure "${verifier_env[@]}" SIGNATURE_CASE=wrong_team "$VERIFIER" "$TEST_DIR/Ashide.app" "$TEST_DIR/Ashide.dmg" ABCDEFGHIJ
expect_failure "${verifier_env[@]}" SIGNATURE_CASE=no_runtime "$VERIFIER" "$TEST_DIR/Ashide.app" "$TEST_DIR/Ashide.dmg" ABCDEFGHIJ
expect_failure "${verifier_env[@]}" STAPLER_FAIL=1 "$VERIFIER" "$TEST_DIR/Ashide.app" "$TEST_DIR/Ashide.dmg" ABCDEFGHIJ
expect_failure "${verifier_env[@]}" APP_GATEKEEPER_FAIL=1 "$VERIFIER" "$TEST_DIR/Ashide.app" "$TEST_DIR/Ashide.dmg" ABCDEFGHIJ
expect_failure "${verifier_env[@]}" DMG_GATEKEEPER_FAIL=1 "$VERIFIER" "$TEST_DIR/Ashide.app" "$TEST_DIR/Ashide.dmg" ABCDEFGHIJ

echo "macOS public distribution contract passed"
