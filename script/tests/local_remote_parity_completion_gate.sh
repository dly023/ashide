#!/bin/bash

set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd)
checker="$repo_root/script/check_local_remote_parity"
tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/ashide-parity-completion-gate.XXXXXX")
trap 'rm -rf "$tmp_dir"' EXIT

tracker="$tmp_dir/tracker.yaml"
matrix="$tmp_dir/capability-matrix.csv"
output="$tmp_dir/output"

write_tracker() {
    local status="$1"
    local gui_or_runtime="$2"

    cat >"$tracker" <<EOF_TRACKER
version: 1
items:
- id: LR-TEST
  status: $status
  verification:
    static_check: passed
    focused_test: passed
    cargo_check: not_applicable
    gui_or_runtime: $gui_or_runtime
EOF_TRACKER
}

write_matrix() {
    local status="$1"

    cat >"$matrix" <<EOF_MATRIX
capability,entry_point,local_path,remote_path,param_parity,timing_parity,status,notes
completion gate fixture,fixture,shared,shared,yes,yes,$status,test fixture
EOF_MATRIX
}

expect_pass() {
    local name="$1"

    if ! "$checker" --completion-gate-only "$tracker" "$matrix" >"$output" 2>&1; then
        echo "[$name] expected completion gate to pass" >&2
        cat "$output" >&2
        exit 1
    fi
}

expect_failure() {
    local name="$1"
    local expected="$2"
    local rc

    set +e
    "$checker" --completion-gate-only "$tracker" "$matrix" >"$output" 2>&1
    rc=$?
    set -e

    if [[ $rc -eq 0 ]]; then
        echo "[$name] expected completion gate to fail closed" >&2
        exit 1
    fi
    if ! grep -Fq -- "$expected" "$output"; then
        echo "[$name] missing diagnostic: $expected" >&2
        cat "$output" >&2
        exit 1
    fi
}

write_tracker verified passed_real_runtime
write_matrix verified
expect_pass verified_contract

# Historical evidence descriptions may mention an earlier pending phase after
# a passed prefix; only an evidence value whose current state starts with
# pending is active.
write_tracker verified passed_after_pending_red_phase
expect_pass historical_pending_word_in_passed_evidence

cat >>"$tracker" <<'EOF_DUPLICATE'
- id: LR-TEST
  status: verified
  verification:
    static_check: passed
    focused_test: passed
    cargo_check: not_applicable
    gui_or_runtime: not_applicable
EOF_DUPLICATE
expect_failure duplicate_tracker_id "duplicate tracker id 'LR-TEST'"

# Tracker evidence children use four-space indentation in the real YAML. This
# fixture must fail even when the item status itself is already verified.
write_tracker verified pending_remote_runtime
write_matrix verified
expect_failure four_space_pending_evidence "pending verification evidence"

# Completion is closed only when every tracker item is verified. A fixed item
# remains active even when every evidence scalar currently says passed.
write_tracker fixed passed_real_runtime
write_matrix verified
expect_failure fixed_tracker_status "status 'fixed' is not completion-terminal"

write_tracker fixed_pending_runtime passed_real_runtime
write_matrix verified
expect_failure unknown_tracker_status "status 'fixed_pending_runtime' is not completion-terminal"

write_tracker verified passed_real_runtime
write_matrix fixed_pending_runtime
expect_failure fixed_pending_runtime_matrix_status "status 'fixed_pending_runtime' is active"

# Matrix active-state detection is allowlist-based so a newly introduced state
# cannot silently bypass a hard-coded regex.
write_matrix partial
expect_failure non_allowlisted_matrix_status "status 'partial' is active"

echo "local/remote parity completion gate contract tests passed"
