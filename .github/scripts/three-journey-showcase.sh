#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GATE="${ROOT}/.github/scripts/three-journey-gate.sh"
RECIPE="${THREETERM_THREE_JOURNEY_RECIPE:-${ROOT}/crates/host/tests/data/bracket_complete_recipe.v1.json}"
RUN_ROOT="${THREETERM_THREE_JOURNEY_ROOT:-${ROOT}/target/three-journey-showcase}"
GRAPHICAL_CONTRACT="${THREETERM_GRAPHICAL_TOOLCHAIN_CONTRACT:-${ROOT}/.github/graphical-toolchain.env}"
LOG_TAIL_PID=''
overall_status=0
api_result='not run'
mcp_result='not run'
tui_result='not run'

usage() {
    cat <<'EOF'
Usage:
  three-journey-showcase.sh

Runs the API, MCP, and TUI all-tools L-bracket journeys, then aggregates their
evidence. The TUI journey opens a fullscreen nested Weston/Ghostty window so
the keyboard-driven modeling workflow is visible on screen.

Run from a Wayland desktop session with the qualified graphical toolchain and
THREETERM_GRAPHICAL_TOOLCHAIN_CONTRACT configured. Evidence defaults to
target/three-journey-showcase and can be relocated with
THREETERM_THREE_JOURNEY_ROOT.
EOF
}

show_stage() {
    local number="$1" title="$2" details="$3"
    if [[ -t 1 && -z "${NO_COLOR:-}" ]]; then
        printf '\n\033[1;36m[%s/3] %s\033[0m\n  %s\n' "$number" "$title" "$details"
    else
        printf '\n[%s/3] %s\n  %s\n' "$number" "$title" "$details"
    fi
}

stream_surface_logs() {
    local surface="$1"
    tail --quiet --follow=name --retry -n +1 \
        "${RUN_ROOT}/process/${surface}.stdout" \
        "${RUN_ROOT}/process/${surface}.stderr" 2>/dev/null &
    LOG_TAIL_PID=$!
}

stop_log_stream() {
    [[ -n "$LOG_TAIL_PID" ]] || return 0
    kill -TERM "$LOG_TAIL_PID" >/dev/null 2>&1 || true
    wait "$LOG_TAIL_PID" >/dev/null 2>&1 || true
    LOG_TAIL_PID=''
}

trap stop_log_stream EXIT INT TERM

if [[ "${1:-}" == --help || "${1:-}" == -h ]]; then
    usage
    exit 0
fi
if (($# > 0)); then
    usage >&2
    exit 2
fi

cd "$ROOT"

if [[ -z "${WAYLAND_DISPLAY:-}" ]]; then
    printf '%s\n' 'visible journey run requires WAYLAND_DISPLAY; launch from a Wayland desktop terminal' >&2
    exit 2
fi
if [[ "$WAYLAND_DISPLAY" == /* ]]; then
    parent_wayland_socket="$WAYLAND_DISPLAY"
elif [[ -n "${XDG_RUNTIME_DIR:-}" ]]; then
    parent_wayland_socket="${XDG_RUNTIME_DIR}/${WAYLAND_DISPLAY}"
else
    printf '%s\n' 'visible journey run requires XDG_RUNTIME_DIR with a Wayland display name' >&2
    exit 2
fi
if [[ ! -S "$parent_wayland_socket" ]]; then
    printf 'Wayland display socket is unavailable: %s\n' "$parent_wayland_socket" >&2
    exit 2
fi
if [[ ! -f "$GRAPHICAL_CONTRACT" ]]; then
    printf 'qualified graphical toolchain contract is missing: %s\n' "$GRAPHICAL_CONTRACT" >&2
    exit 2
fi
if [[ ! -f "$RECIPE" ]]; then
    printf 'shared all-tools recipe is missing: %s\n' "$RECIPE" >&2
    exit 2
fi

if [[ -z "${THREETERM_JOURNEY_RUN_ID:-}" ]]; then
    THREETERM_JOURNEY_RUN_ID="$(git rev-parse HEAD)-$(date +%s%N)"
fi
export THREETERM_JOURNEY_RUN_ID
export THREETERM_THREE_JOURNEY_ROOT="$RUN_ROOT"
export THREETERM_GRAPHICAL_VISIBLE=true
export THREETERM_REQUIRE_OCCT=1
export THREETERM_REQUIRE_REAL_WORKER=1
export THREETERM_REQUIRE_IMMUTABLE_WORKERS=1
unset THREETERM_THREE_JOURNEY_RESET

printf '%s\n' 'ThreeTerm — visible three-journey run'
printf 'Run ID: %s\nEvidence: %s\n' "$THREETERM_JOURNEY_RUN_ID" "$RUN_ROOT"
printf '%s\n' 'Recipe: 33 operations from a new bracket through reinforcement, save, replay, and STL export.'
printf '%s\n' 'The API and MCP journeys are headless; their live test output is streamed here.'
printf '%s\n' 'The TUI journey opens a fullscreen Weston/Ghostty window with its keyboard-driven actions visible.'

run_surface() {
    local surface="$1" number="$2" title="$3" details="$4" status gate_pid stdout_path
    show_stage "$number" "$title" "$details"
    stdout_path="${RUN_ROOT}/process/${surface}.stdout"
    if [[ "$surface" == api ]]; then
        THREETERM_THREE_JOURNEY_RESET=true bash "$GATE" --surface "$surface" &
    else
        bash "$GATE" --surface "$surface" &
    fi
    gate_pid=$!
    while [[ ! -f "$stdout_path" ]] && kill -0 "$gate_pid" >/dev/null 2>&1; do
        sleep 0.05
    done
    stream_surface_logs "$surface"
    if wait "$gate_pid"; then
        status=0
    else
        status=$?
    fi
    stop_log_stream
    if ((status == 0)); then
        printf '  ✓ %s journey passed\n' "$surface"
        printf -v "${surface}_result" '%s' 'passed'
    else
        printf '  ✗ %s journey failed (see %s/process/%s.stderr)\n' \
            "$surface" "$RUN_ROOT" "$surface" >&2
        printf -v "${surface}_result" '%s' 'failed'
        overall_status=1
    fi
}

run_surface api 1 'API — public command journey' \
    'Build and qualify the 33-step bracket via the versioned host API; verify committed BREP geometry, canonical transaction intents, replay, and exported STL.'
run_surface mcp 2 'MCP — JSON-RPC tool journey' \
    'Discover the registered tools over stdio, run the same recipe through MCP, validate schemas and geometry, and check invalid-edit recovery.'
run_surface tui 3 'TUI — visible Ghostty modeling journey' \
    'Watch the new fullscreen test window: command palette, draft previews, commits, feature selection, reinforcement, save/reopen, and STL export.'

printf '\nAggregating all three Journey Evidence Reports…\n'
if bash "$GATE" --aggregate; then
    aggregate_result='passed'
else
    aggregate_result='failed'
    overall_status=1
fi

printf '\nJourney summary: API=%s MCP=%s TUI=%s aggregate=%s\n' \
    "$api_result" "$mcp_result" "$tui_result" "$aggregate_result"
printf 'Catalog: %s/catalog.json\n' "$RUN_ROOT"
printf 'Per-surface logs: %s/process/\n' "$RUN_ROOT"
exit "$overall_status"
