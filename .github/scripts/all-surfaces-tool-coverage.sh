#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
EVIDENCE_ROOT="${THREETERM_COVERAGE_EVIDENCE_ROOT:-${CARGO_TARGET_DIR:-${ROOT}/target}/journey-coverage}"
API_REPORT=''
MCP_REPORT=''
TUI_REPORT=''

usage() {
    cat <<'EOF'
Usage: all-surfaces-tool-coverage.sh [--evidence-root PATH]
       [--api-report PATH] [--mcp-report PATH] [--tui-report PATH]
EOF
}

while (($# > 0)); do
    case "$1" in
        --evidence-root)
            [[ $# -ge 2 ]] || { usage >&2; exit 2; }
            EVIDENCE_ROOT="$2"
            shift 2
            ;;
        --api-report)
            [[ $# -ge 2 ]] || { usage >&2; exit 2; }
            API_REPORT="$2"
            shift 2
            ;;
        --mcp-report)
            [[ $# -ge 2 ]] || { usage >&2; exit 2; }
            MCP_REPORT="$2"
            shift 2
            ;;
        --tui-report)
            [[ $# -ge 2 ]] || { usage >&2; exit 2; }
            TUI_REPORT="$2"
            shift 2
            ;;
        --help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            exit 2
            ;;
    esac
done

API_REPORT="${API_REPORT:-${EVIDENCE_ROOT}/api-journey-coverage.json}"
MCP_REPORT="${MCP_REPORT:-${EVIDENCE_ROOT}/mcp-journey-coverage.json}"
TUI_REPORT="${TUI_REPORT:-${EVIDENCE_ROOT}/tui-journey-coverage.json}"

mkdir -p "$EVIDENCE_ROOT"
stage_report() {
    local source="$1"
    local destination="$2"
    [[ -f "$source" ]] || {
        printf 'coverage: retained report is missing: %s\n' "$source" >&2
        rm -f -- "$destination"
        return 0
    }
    if [[ "$(realpath "$source")" != "$(realpath -m "$destination")" ]]; then
        cp -- "$source" "${destination}.tmp.$$"
        mv -f -- "${destination}.tmp.$$" "$destination"
    fi
}

stage_report "$API_REPORT" "${EVIDENCE_ROOT}/api-journey-coverage.json"
stage_report "$MCP_REPORT" "${EVIDENCE_ROOT}/mcp-journey-coverage.json"
stage_report "$TUI_REPORT" "${EVIDENCE_ROOT}/tui-journey-coverage.json"

export THREETERM_COVERAGE_EVIDENCE_ROOT="$EVIDENCE_ROOT"
exec cargo test -p threeterm-protocol --test coverage all_surfaces_tool_coverage_matrix \
    --jobs 1 -- --include-ignored --exact --nocapture
