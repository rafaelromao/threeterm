#!/usr/bin/env bash

set -euo pipefail

ROOT="$(dirname "$0")/.."
TEST_SUITE="${ROOT}/.github/scripts/test-suite.sh"
TEMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TEMP_DIR}"' EXIT

printf '%s\n' \
    '#!/usr/bin/env bash' \
    'printf "%s\n" "$*" > "${THREETERM_CARGO_ARGS}"' \
    > "${TEMP_DIR}/cargo"
chmod +x "${TEMP_DIR}/cargo"

run_selector() {
    PATH="${TEMP_DIR}:${PATH}" THREETERM_CARGO_ARGS="${TEMP_DIR}/args" \
        bash "${TEST_SUITE}" "$1"
    ACTUAL="$(<"${TEMP_DIR}/args")"
    if [ "${ACTUAL}" != "$2" ]; then
        echo "expected cargo $2, got $ACTUAL" >&2
        exit 1
    fi
}

run_selector fast "test --workspace -- --skip supervised_occt_extrude --skip supervised_occt_replay --skip canonical_extrude_replay --skip real_occt_geometry_smoke --skip adapter_command_parity --skip required_occt_worker --skip historical_named_revision_restore --skip historical_recovery_adapter_parity --skip generation_identity --skip generation_publication --skip generation_interruption_recovery --skip reusable_geometry_artifact_discard_replay --skip reusable_geometry_adapter_parity --skip reusable_geometry_divergence --skip interactive_shared_command_semantics --skip interactive_production_event_loop"
run_selector slow "test --workspace --jobs 1 -- --ignored --skip production_tui_ghostty_session --skip production_tui_create_project_extrude --skip production_tui_keyboard_navigation --skip production_tui_mirror_pattern_reinforcing_features --skip production_tui_tapered_lofted_reinforcements --skip production_tui_bracket_foundation --test-threads=1"
run_selector e2e "test --workspace --jobs 1 -- --include-ignored --skip production_tui_ghostty_session --skip production_tui_create_project_extrude --skip production_tui_keyboard_navigation --skip production_tui_mirror_pattern_reinforcing_features --skip production_tui_tapered_lofted_reinforcements --skip production_tui_bracket_foundation --test-threads=1"
run_selector graphical "test -p threeterm-tui --test graphical_launch --jobs 1 -- --ignored --test-threads=1"

if PATH="${TEMP_DIR}:${PATH}" bash "${TEST_SUITE}" unsupported >/dev/null 2>&1; then
    echo "unsupported suite unexpectedly succeeded" >&2
    exit 1
fi
