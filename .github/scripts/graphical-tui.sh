#!/usr/bin/env bash
set -euo pipefail

SCHEMA_VERSION='threeterm.graphical-tui/1'
TRANSIENT_EMPTY_PROJECT_SOURCE_REVISION='empty-project'
PERSISTED_TRANSIENT_SOURCE_REVISION='empty-session-source'
TEST_ID='production_tui_ghostty_session'
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TOOLCHAIN_CONTRACT="${THREETERM_GRAPHICAL_TOOLCHAIN_CONTRACT:-${ROOT}/.github/graphical-toolchain.env}"
COMPOSITOR_WIDTH=800
COMPOSITOR_HEIGHT=600
TERMINAL_COLUMNS=80
TERMINAL_ROWS=24
VIEWPORT_CROP='0,0,800x480'
LOCALE='C.UTF-8'
RUNNER_TIMEOUT_SECONDS="${THREETERM_GRAPHICAL_TIMEOUT_SECONDS:-60}"
PROBE_STIMULUS_SECONDS="${THREETERM_GRAPHICAL_PROBE_STIMULUS_SECONDS:-2}"

TUI_BINARY=''
PROJECT_ROOT=''
EVIDENCE_ROOT=''
PRINT_PLAN=0
VALIDATE_EVIDENCE=''

WESTON_PID=''
GHOSTTY_PID=''
STIMULUS_PID=''
WAYLAND_DISPLAY=''
XDG_RUNTIME_DIR=''
OUTPUT_NAME=''
WRAPPER=''
TUI_STATUS_FILE=''
PTY_OUTPUT=''
PTY_INPUT=''
TUI_STDERR=''
WESTON_LOG=''
STARTUP_SCREENSHOT=''
ORBIT_SCREENSHOT=''
STARTUP_VIEWPORT_CROP=''
SELECTION_SCREENSHOT=''
PAN_SCREENSHOT=''
ZOOM_SCREENSHOT=''
MIRROR_SCREENSHOT=''
LINEAR_PATTERN_SCREENSHOT=''
CIRCULAR_PATTERN_SCREENSHOT=''
SELECTION_VIEWPORT_CROP=''
ORBIT_VIEWPORT_CROP=''
PAN_VIEWPORT_CROP=''
ZOOM_VIEWPORT_CROP=''
NAVIGATION_TRANSCRIPT=''
REINFORCING_TRANSCRIPT=''
PROJECT_CREATED_SCREENSHOT=''
EXTRUSION_COMMITTED_SCREENSHOT=''
PROJECT_IDENTITY=''
CLEANUP_SCREENSHOT=''
FAILURE_SCREENSHOT=''
DIFF_LOG=''
TOOL_VERSIONS=''
MANIFEST=''
STIMULUS_ERROR=''
success=0
failure_code=''
failure_detail=''
readiness_detail=''
viewport_evidence='null'
viewport_startup_evidence='null'
viewport_orbit_evidence='null'
viewport_workflow_evidence='null'
viewport_selection_evidence='null'
viewport_pan_evidence='null'
viewport_zoom_evidence='null'
startup_image_id=''
startup_revision=''
final_image_id=''
final_image_id_json='null'
final_delete_image_id=''
final_delete_image_id_json='null'
cleanup_deletions='[]'
probe_status='not_run'
readiness_status='not_run'
orbit_status='not_run'
navigation_status='not_run'
workflow_status='not_run'
cleanup_status='not_run'
ghostty_status='not_run'
tui_status='not_run'
weston_status='not_run'
owned_processes_status='not_run'
cleanup_screenshot_taken=0
source_commit='unknown'
source_dirty=false
navigation_project_generation_digest_before=''
navigation_project_generation_digest_after=''
navigation_previous_image_id=''
navigation_previous_viewport_crop_sha256=''
reinforcing_viewport_phase=0
path_failure_code=''
path_failure_detail=''
EXPECTED_FEATURE_ID='l-bracket'
CREATED_PROJECT_ROOT=''
OCCT_WORKER=''

usage() {
    cat <<'EOF'
Usage:
  graphical-tui.sh production_tui_ghostty_session --tui-binary PATH --project-root PATH --evidence-root PATH
  graphical-tui.sh production_tui_create_project_extrude --tui-binary PATH --project-root PATH --evidence-root PATH
  graphical-tui.sh production_tui_keyboard_navigation --tui-binary PATH --project-root PATH --evidence-root PATH
  graphical-tui.sh production_tui_mirror_pattern_reinforcing_features --tui-binary PATH --project-root PATH --evidence-root PATH
  graphical-tui.sh --print-plan
  graphical-tui.sh --validate-viewport-evidence PATH

The named test requires a qualified direct-Ghostty graphical environment. It
never spoofs TERM or TERM_PROGRAM and never passes when prerequisites are absent.
EOF
}

print_plan() {
    cat <<EOF
{
  "schema_version": "$SCHEMA_VERSION",
  "result": "not_run",
  "test": "$TEST_ID",
  "configuration": {
    "locale": "C.UTF-8",
    "palette": "catppuccin",
    "compositor": {"width": 800, "height": 600},
    "terminal": {"columns": 80, "rows": 24},
    "viewport_crop": "0,0,800x480"
  },
  "requirements": [
    "weston", "ghostty", "wtype", "ydotool", "wlr-randr", "grim",
    "tesseract", "magick", "jq", "sha256sum", "script", "setsid", "timeout"
  ]
}
EOF
}

die() {
    failure_code="$1"
    failure_detail="$2"
    exit 1
}

validate_viewport_evidence() {
    local payload="$1"
    local reinforcing=false
    local reinforcing_test=false
    if [[ "$TEST_ID" == 'production_tui_mirror_pattern_reinforcing_features' ]]; then
        reinforcing_test=true
        [[ "$reinforcing_viewport_phase" -gt 0 ]] && reinforcing=true
    fi
    jq -e \
        --arg expected_feature "$EXPECTED_FEATURE_ID" \
        --argjson reinforcing "$reinforcing" \
        --argjson reinforcing_test "$reinforcing_test" \
        --argjson reinforcing_phase "$reinforcing_viewport_phase" '
        .schema_version == "threeterm.viewport-evidence/1" and
        .acknowledgement == "viewport-presented" and
        (.frame.frame_token | type == "number" and . > 0) and
        (.frame.image_id | type == "number" and . > 0) and
        (.frame.generation | type == "number") and
        (.frame.revision | type == "string" and length > 0) and
        .frame.width == 800 and .frame.height == 480 and
        (if $reinforcing then
            (.scene.solids | type == "array" and
                (if $reinforcing_phase == 3 then
                    length == 3 and (map(.feature_id) | sort) == ["l-bracket", "reinforce-pad", "tui-mirror-pad"]
                 elif $reinforcing_phase == 4 then
                    length == 4 and (map(.feature_id) | sort) == ["l-bracket", "reinforce-pad", "tui-linear-pads", "tui-mirror-pad"]
                 else
                    length == 5 and (map(.feature_id) | sort) == ["l-bracket", "reinforce-pad", "tui-circular-lugs", "tui-linear-pads", "tui-mirror-pad"]
                 end) and all(.[]; .triangle_count > 0))
         else
            (.scene.solids | type == "array" and
                (if $reinforcing_test and $reinforcing == false then
                    length == 2 and (map(.feature_id) | sort) == ["l-bracket", "reinforce-pad"]
                 else
                    length == 1 and .[0].feature_id == $expected_feature and .[0].triangle_count > 0
                 end) and all(.[]; .triangle_count > 0))
         end) and
        (.scene.triangle_count == (.scene.solids | map(.triangle_count) | add)) and
        (.scene.triangle_count | type == "number" and . > 0) and
        (.scene.body_pixels | type == "number" and . > 0) and
        (.scene.edge_pixels | type == "number" and . > 0) and
        (.scene.non_background_pixels | type == "number" and . > 0) and
        .palette.name == "catppuccin" and
        (.camera.yaw_degrees | type == "number") and
        (.camera.pitch_degrees | type == "number") and
        (.camera.zoom_percent | type == "number") and
        (.camera.pan_x | type == "number") and
        (.camera.pan_y | type == "number") and
        .palette.colors == {
            "background": [29,29,45],
            "body": [125,125,152],
            "edge": [198,165,162],
            "grid": [119,155,149],
            "selected_body": [192,193,222],
            "selected_edge": [121,111,136],
            "candidate_body": [164,153,179],
            "candidate_edge": [221,207,180],
            "drag_feedback": [146,167,189],
            "overlay": [155,132,152],
            "warning": [235,220,193],
            "error": [208,174,171]
        }
    ' <<<"$payload" >/dev/null 2>&1
}

validate_empty_viewport_evidence() {
    local payload="$1"
    jq -e --arg expected_revision "$TRANSIENT_EMPTY_PROJECT_SOURCE_REVISION" '
        .schema_version == "threeterm.viewport-evidence/1" and
        .acknowledgement == "viewport-presented" and
        (.frame.frame_token | type == "number" and . > 0) and
        (.frame.image_id | type == "number" and . > 0) and
        (.frame.generation | type == "number") and
        .frame.revision == $expected_revision and
        .frame.width == 800 and .frame.height == 480 and
        (.scene.solids | type == "array" and length == 0) and
        (.scene.triangle_count | type == "number" and . == 0) and
        .scene.body_pixels == 0 and
        .scene.edge_pixels == 0 and
        .scene.non_background_pixels == 0 and
        .palette.name == "catppuccin"
    ' <<<"$payload" >/dev/null 2>&1
}

validate_selected_viewport_evidence() {
    local payload="$1"
    local expected_feature="$2"
    validate_viewport_evidence "$payload" || return 1
    jq -e --arg expected_feature "$expected_feature" '
        .selected_feature_id == $expected_feature
    ' <<<"$payload" >/dev/null 2>&1
}

while (($# > 0)); do
    case "$1" in
        --help|-h)
            usage
            exit 0
            ;;
        --print-plan)
            PRINT_PLAN=1
            shift
            ;;
        --validate-viewport-evidence)
            (($# >= 2)) || { usage >&2; exit 2; }
            VALIDATE_EVIDENCE="$2"
            shift 2
            ;;
        --tui-binary)
            (($# >= 2)) || { usage >&2; exit 2; }
            TUI_BINARY="$2"
            shift 2
            ;;
        --project-root)
            (($# >= 2)) || { usage >&2; exit 2; }
            PROJECT_ROOT="$2"
            shift 2
            ;;
        --evidence-root)
            (($# >= 2)) || { usage >&2; exit 2; }
            EVIDENCE_ROOT="$2"
            shift 2
            ;;
        production_tui_ghostty_session|production_tui_create_project_extrude|production_tui_keyboard_navigation|production_tui_mirror_pattern_reinforcing_features)
            [[ -z "${TEST_NAME:-}" ]] || { usage >&2; exit 2; }
            TEST_NAME="$1"
            if [[ "$TEST_NAME" == 'production_tui_create_project_extrude' ]]; then
                TEST_ID="$TEST_NAME"
                SCHEMA_VERSION='threeterm.graphical-tui.create-project-extrude/1'
            elif [[ "$TEST_NAME" == 'production_tui_keyboard_navigation' ]]; then
                TEST_ID="$TEST_NAME"
                SCHEMA_VERSION='threeterm.graphical-tui.keyboard-navigation/1'
            elif [[ "$TEST_NAME" == 'production_tui_mirror_pattern_reinforcing_features' ]]; then
                TEST_ID="$TEST_NAME"
                SCHEMA_VERSION='threeterm.graphical-tui.mirror-pattern-reinforcing-features/1'
            fi
            shift
            ;;
        *)
            usage >&2
            exit 2
            ;;
    esac
done

if ((PRINT_PLAN)); then
    (($# == 0)) || { usage >&2; exit 2; }
    print_plan
    exit 0
fi

if [[ -n "$VALIDATE_EVIDENCE" ]]; then
    [[ -f "$VALIDATE_EVIDENCE" ]] || exit 2
    validate_viewport_evidence "$(<"$VALIDATE_EVIDENCE")"
    exit $?
fi

[[ "${TEST_NAME:-}" == "$TEST_ID" ]] || { usage >&2; exit 2; }
[[ -n "$TUI_BINARY" && -n "$PROJECT_ROOT" && -n "$EVIDENCE_ROOT" ]] || {
    usage >&2
    exit 2
}

mkdir -p "$EVIDENCE_ROOT" || {
    printf '{"schema_version":"%s","result":"failed","integrity":"unavailable","failure":{"code":"evidence_root_unavailable"}}\n' "$SCHEMA_VERSION" >&2
    exit 1
}
EVIDENCE_ROOT="$(cd "$EVIDENCE_ROOT" && pwd)"
if [[ "$TEST_ID" == 'production_tui_create_project_extrude' ]]; then
    project_parent=''
    if project_parent="$(cd "$(dirname "$PROJECT_ROOT")" 2>/dev/null && pwd)"; then
        PROJECT_ROOT="${project_parent}/$(basename "$PROJECT_ROOT")"
        CREATED_PROJECT_ROOT="${PROJECT_ROOT}-created"
    else
        path_failure_code='project_root_unavailable'
        path_failure_detail='fresh project launch root parent is not a directory'
        PROJECT_ROOT=''
    fi
elif PROJECT_ROOT="$(cd "$PROJECT_ROOT" 2>/dev/null && pwd)"; then
    :
else
    path_failure_code='project_root_unavailable'
    path_failure_detail='project root is not a directory'
    PROJECT_ROOT=''
fi
if TUI_BINARY="$(cd "$(dirname "$TUI_BINARY")" 2>/dev/null && pwd)/$(basename "$TUI_BINARY")"; then
    :
else
    path_failure_code='tui_binary_unavailable'
    path_failure_detail='TUI binary parent is unavailable'
    TUI_BINARY=''
fi

MANIFEST="${EVIDENCE_ROOT}/manifest.json"
PTY_OUTPUT="${EVIDENCE_ROOT}/pty-output.log"
PTY_INPUT="${EVIDENCE_ROOT}/pty-input.log"
TUI_STDERR="${EVIDENCE_ROOT}/tui-stderr.log"
WESTON_LOG="${EVIDENCE_ROOT}/weston.log"
TOOL_VERSIONS="${EVIDENCE_ROOT}/tool-versions.json"
if [[ "$TEST_ID" == 'production_tui_create_project_extrude' ]]; then
    STARTUP_SCREENSHOT="${EVIDENCE_ROOT}/empty-startup.png"
    PROJECT_CREATED_SCREENSHOT="${EVIDENCE_ROOT}/project-created.png"
    EXTRUSION_COMMITTED_SCREENSHOT="${EVIDENCE_ROOT}/extrusion-committed.png"
else
    STARTUP_SCREENSHOT="${EVIDENCE_ROOT}/startup.png"
    PROJECT_CREATED_SCREENSHOT=''
    EXTRUSION_COMMITTED_SCREENSHOT=''
fi
ORBIT_SCREENSHOT="${EVIDENCE_ROOT}/orbit.png"
if [[ "$TEST_ID" == 'production_tui_keyboard_navigation' ]]; then
    STARTUP_VIEWPORT_CROP="${EVIDENCE_ROOT}/startup-viewport.png"
    SELECTION_SCREENSHOT="${EVIDENCE_ROOT}/selection.png"
    PAN_SCREENSHOT="${EVIDENCE_ROOT}/pan.png"
    ZOOM_SCREENSHOT="${EVIDENCE_ROOT}/zoom.png"
    SELECTION_VIEWPORT_CROP="${EVIDENCE_ROOT}/selection-viewport.png"
    ORBIT_VIEWPORT_CROP="${EVIDENCE_ROOT}/orbit-viewport.png"
    PAN_VIEWPORT_CROP="${EVIDENCE_ROOT}/pan-viewport.png"
    ZOOM_VIEWPORT_CROP="${EVIDENCE_ROOT}/zoom-viewport.png"
    NAVIGATION_TRANSCRIPT="${EVIDENCE_ROOT}/navigation-transcript.jsonl"
fi
if [[ "$TEST_ID" == 'production_tui_mirror_pattern_reinforcing_features' ]]; then
    MIRROR_SCREENSHOT="${EVIDENCE_ROOT}/mirror.png"
    LINEAR_PATTERN_SCREENSHOT="${EVIDENCE_ROOT}/linear-pattern.png"
    CIRCULAR_PATTERN_SCREENSHOT="${EVIDENCE_ROOT}/circular-pattern.png"
    REINFORCING_TRANSCRIPT="${EVIDENCE_ROOT}/reinforcing-transcript.jsonl"
fi
if [[ "$TEST_ID" == 'production_tui_create_project_extrude' ]]; then
    PROJECT_IDENTITY="${EVIDENCE_ROOT}/project-identity.json"
else
    PROJECT_IDENTITY=''
fi
CLEANUP_SCREENSHOT="${EVIDENCE_ROOT}/cleanup.png"
FAILURE_SCREENSHOT="${EVIDENCE_ROOT}/failure.png"
DIFF_LOG="${EVIDENCE_ROOT}/orbit-difference.txt"
STIMULUS_ERROR="${EVIDENCE_ROOT}/probe-stimulus-error.txt"
XDG_RUNTIME_DIR="${EVIDENCE_ROOT}/runtime"
WAYLAND_DISPLAY="threeterm-${BASHPID}.wayland"
export LC_ALL LANG XDG_RUNTIME_DIR WAYLAND_DISPLAY

if git -C "$ROOT" rev-parse HEAD >/dev/null 2>&1; then
    source_commit="$(git -C "$ROOT" rev-parse HEAD)"
    if git -C "$ROOT" diff --quiet && git -C "$ROOT" diff --cached --quiet; then
        source_dirty=false
    else
        source_dirty=true
    fi
fi

json_escape_minimal() {
    local value="$1"
    value="${value//\\/\\\\}"
    value="${value//\"/\\\"}"
    value="${value//$'\r'/\\r}"
    value="${value//$'\n'/\\n}"
    printf '%s' "$value"
}

write_minimal_manifest() {
    local escaped_code escaped_detail
    escaped_code="$(json_escape_minimal "${failure_code:-runner_failure}")"
    escaped_detail="$(json_escape_minimal "${failure_detail:-graphical runner failed}")"
    printf '{"schema_version":"%s","result":"failed","test":"%s","integrity":"unavailable","configuration":{"locale":"%s","compositor":{"width":%d,"height":%d},"terminal":{"columns":%d,"rows":%d},"viewport_crop":"%s"},"failure":{"code":"%s","detail":"%s"},"artifacts":[]}\n' \
        "$SCHEMA_VERSION" "$TEST_ID" "$LOCALE" "$COMPOSITOR_WIDTH" "$COMPOSITOR_HEIGHT" \
        "$TERMINAL_COLUMNS" "$TERMINAL_ROWS" "$VIEWPORT_CROP" "$escaped_code" "$escaped_detail" \
        >"${MANIFEST}.tmp.$$" && mv -f "${MANIFEST}.tmp.$$" "$MANIFEST"
}

tool_output() {
    local name="$1"
    local command_name="$2"
    local output
    output="$($command_name --version 2>&1 || true)"
    printf '%s\n' "$output" >"${EVIDENCE_ROOT}/tool-${name}.txt"
    printf '%s' "$output"
}

contract_value() {
    local key="$1"
    [[ -f "$TOOLCHAIN_CONTRACT" ]] || return 1
    while IFS='=' read -r name value; do
        [[ "$name" == "$key" ]] && {
            printf '%s' "$value"
            return 0
        }
    done <"$TOOLCHAIN_CONTRACT"
    return 1
}

check_prerequisites() {
    local missing=()
    local command_name
    local -a commands=(weston ghostty wtype ydotool wlr-randr grim tesseract magick jq sha256sum script setsid timeout)
    for command_name in "${commands[@]}"; do
        command -v "$command_name" >/dev/null 2>&1 || missing+=("$command_name")
    done
    ((${#missing[@]} == 0)) || die prerequisite_missing "missing graphical prerequisites: ${missing[*]}"
    [[ -f "$TOOLCHAIN_CONTRACT" ]] || die toolchain_contract_missing "exact graphical toolchain contract is missing: $TOOLCHAIN_CONTRACT"

    local -A contract_keys=(
        [weston]=WESTON_VERSION
        [ghostty]=GHOSTTY_VERSION
        [wtype]=WTYPE_VERSION
        [ydotool]=YDTOOL_VERSION
        [wlr-randr]=WLR_RANDR_VERSION
        [grim]=GRIM_VERSION
        [tesseract]=TESSERACT_VERSION
        [magick]=MAGICK_VERSION
        [jq]=JQ_VERSION
        [sha256sum]=SHA256SUM_VERSION
        [script]=SCRIPT_VERSION
        [setsid]=SETSID_VERSION
        [timeout]=TIMEOUT_VERSION
    )
    local -A outputs=()
    local expected output
    for command_name in "${commands[@]}"; do
        expected="$(contract_value "${contract_keys[$command_name]}")" ||
            die toolchain_contract_invalid "toolchain contract has no exact output for $command_name"
        if [[ "$command_name" == ghostty && "$expected" != *'1.3.1-arch2'* ]]; then
            die toolchain_contract_invalid 'Ghostty must be pinned to version 1.3.1-arch2'
        fi
        output="$(tool_output "$command_name" "$command_name")"
        outputs["$command_name"]="$output"
        [[ "$output" == *"$expected"* ]] ||
            die toolchain_version_mismatch "$command_name version does not contain the contracted output: $expected"
    done
    local backend
    backend="$(contract_value WESTON_HEADLESS_BACKEND)" ||
        die toolchain_contract_invalid 'toolchain contract has no Weston headless backend'
    [[ "$backend" == 'headless-backend.so' ]] ||
        die toolchain_contract_invalid "unsupported Weston backend: $backend"

    if [[ "$TEST_ID" == 'production_tui_create_project_extrude' ]]; then
        OCCT_WORKER="${THREETERM_OCCTBUILD_WORKER:-}"
        if [[ -z "$OCCT_WORKER" ]]; then
            local target_root="${CARGO_TARGET_DIR:-${ROOT}/target}"
            local candidate
            for candidate in \
                "$target_root/debug/bin/threeterm-occt-worker" \
                "$target_root"/debug/build/threeterm-occt-*/out/bin/threeterm-occt-worker; do
                if [[ -f "$candidate" ]]; then
                    OCCT_WORKER="$candidate"
                    break
                fi
            done
        fi
        [[ -x "$OCCT_WORKER" ]] ||
            die occt_worker_unavailable 'qualified fresh workflow requires an executable OCCT worker'
    fi

    local contract_hash=''
    if [[ -x "$(command -v sha256sum)" ]]; then
        contract_hash="$(sha256sum "$TOOLCHAIN_CONTRACT" | cut -d' ' -f1)"
    fi
    local json='{}'
    local tool
    for tool in "${commands[@]}"; do
        json="$(jq --arg name "$tool" --arg output "${outputs[$tool]}" '. + {($name): $output}' <<<"$json")"
    done
    jq -n \
        --arg contract "$TOOLCHAIN_CONTRACT" \
        --arg contract_sha256 "$contract_hash" \
        --argjson outputs "$json" \
        '{contract: $contract, contract_sha256: (if $contract_sha256 == "" then null else $contract_sha256 end), outputs: $outputs}' \
        >"$TOOL_VERSIONS"
}

capture_screenshot() {
    local destination="$1"
    grim "$destination" >/dev/null 2>&1 || return 1
    [[ "$(magick identify -format '%wx%h' "$destination" 2>/dev/null)" == '800x600' ]] || return 1
    return 0
}

wait_until() {
    local seconds="$1"
    shift
    local deadline=$((SECONDS + seconds))
    while ((SECONDS < deadline)); do
        "$@" && return 0
        sleep 0.1
    done
    return 1
}

wayland_ready() {
    [[ -S "${XDG_RUNTIME_DIR}/${WAYLAND_DISPLAY}" ]]
}

screen_ready() {
    capture_screenshot "${EVIDENCE_ROOT}/window-ready.png"
}

readiness_ready() {
    local line capability_evidence=''
    while IFS= read -r line; do
        [[ "$line" == *'[ready-status] Interactive Modeling ready probe='* ]] || continue
        capability_evidence="${line#* probe=}"
    done < <(tr '\r' '\n' <"$PTY_OUTPUT" 2>/dev/null || true)
    grep -aFq '[ready-status] Interactive Modeling ready' "$PTY_OUTPUT" 2>/dev/null || return 1
    grep -aFq ';OK' "$PTY_INPUT" 2>/dev/null || return 1
    grep -aFq 'a=T,t=d' "$PTY_OUTPUT" 2>/dev/null || return 1
    [[ -n "$capability_evidence" ]] || return 1
    if ! jq -e '
        .state == "valid" and
        .direct_ghostty and
        .kitty_rgb_zlib and
        .kitty_acknowledgements and
        .kitty_keyboard and
        .sgr_mouse_cell and
        .sgr_mouse_pixel and
        .focus_reporting and
        .alternate_screen and
        .resize_events
    ' <<<"$capability_evidence" >/dev/null 2>&1; then
        readiness_detail="capability evidence did not establish a valid direct interactive Ghostty attachment: ${capability_evidence}"
        return 1
    fi
    readiness_viewport_ready || return 1
    return 0
}

extract_viewport_evidence() {
    local line
    viewport_evidence=''
    while IFS= read -r line; do
        [[ "$line" == *'[viewport-status] Viewport presented '* ]] || continue
        viewport_evidence="${line#* Viewport presented }"
    done < <(tr '\r' '\n' <"$PTY_OUTPUT" 2>/dev/null || true)
    [[ -n "$viewport_evidence" ]]
}

evidence_wire_ready() {
    local image_id="$1"
    grep -aFq "i=${image_id},o=z" "$PTY_OUTPUT" 2>/dev/null || return 1
    grep -aFq "i=${image_id};OK" "$PTY_INPUT" 2>/dev/null
}

readiness_viewport_ready() {
    extract_viewport_evidence || return 1
    if [[ "$TEST_ID" == 'production_tui_create_project_extrude' ]]; then
        validate_empty_viewport_evidence "$viewport_evidence" || {
            readiness_detail="viewport evidence did not satisfy the empty-project contract: ${viewport_evidence}"
            return 1
        }
    elif ! validate_viewport_evidence "$viewport_evidence"; then
        readiness_detail="viewport evidence did not satisfy the versioned saved-solid contract: ${viewport_evidence}"
        return 1
    fi
    [[ "$(jq -r '.camera.yaw_degrees' <<<"$viewport_evidence")" == 0 ]] || return 1
    [[ "$(jq -r '.camera.pitch_degrees' <<<"$viewport_evidence")" == 20 ]] || return 1
    [[ "$(jq -r '.camera.zoom_percent' <<<"$viewport_evidence")" == 100 ]] || return 1
    [[ "$(jq -r '.camera.pan_x' <<<"$viewport_evidence")" == 0 ]] || return 1
    [[ "$(jq -r '.camera.pan_y' <<<"$viewport_evidence")" == 0 ]] || return 1
    startup_image_id="$(jq -r '.frame.image_id' <<<"$viewport_evidence")"
    startup_revision="$(jq -r '.frame.revision' <<<"$viewport_evidence")"
    evidence_wire_ready "$startup_image_id" || return 1
    viewport_startup_evidence="$viewport_evidence"
}

rgb_pixel_count() {
    local image="$1"
    local red="$2"
    local green="$3"
    local blue="$4"
    local count
    count="$(magick "$image" -crop 800x480+0+0 \
        -fx "(floor(r*255+0.5)==${red} && floor(g*255+0.5)==${green} && floor(b*255+0.5)==${blue}) ? 1 : 0" \
        -format '%[fx:mean*w*h]' info: 2>/dev/null || true)"
    [[ "$count" =~ ^[0-9]+([.][0-9]+)?$ ]] || return 1
    printf '%.0f' "$count"
}

rendered_viewport_ready() {
    local screenshot="$1"
    local body_pixels edge_pixels
    body_pixels="$(rgb_pixel_count "$screenshot" 125 125 152)" || return 1
    edge_pixels="$(rgb_pixel_count "$screenshot" 198 165 162)" || return 1
    [[ "$body_pixels" -ge 100 && "$edge_pixels" -ge 10 ]]
}

rendered_selected_viewport_ready() {
    local screenshot="$1"
    local selected_body selected_edge
    selected_body="$(rgb_pixel_count "$screenshot" 192 193 222)" || return 1
    selected_edge="$(rgb_pixel_count "$screenshot" 121 111 136)" || return 1
    [[ "$selected_body" -ge 100 && "$selected_edge" -ge 10 ]]
}

project_generation_digest() {
    local root="$1" path
    [[ -d "$root" ]] || return 1
    (
        cd "$root" || exit 1
        while IFS= read -r -d '' path; do
            if [[ -d "$path" ]]; then
                printf 'directory:%s\n' "$path"
            elif [[ -f "$path" ]]; then
                printf 'file:%s:' "$path"
                sha256sum -- "$path"
            else
                printf 'other:%s\n' "$path"
            fi
        done < <(find . -mindepth 1 -print0 | LC_ALL=C sort -z)
    ) | sha256sum | cut -d' ' -f1
}

orbit_ready() {
    grep -aFq '[motion-trail] Orbit right' "$PTY_OUTPUT" 2>/dev/null || return 1
    grep -aFq 'a=T,t=d' "$PTY_OUTPUT" 2>/dev/null || return 1
    extract_viewport_evidence || return 1
    validate_viewport_evidence "$viewport_evidence" || return 1
    if [[ "$TEST_ID" == 'production_tui_create_project_extrude' ||
        "$TEST_ID" == 'production_tui_mirror_pattern_reinforcing_features' ]]; then
        [[ "$(jq -r '.frame.revision' <<<"$viewport_evidence")" == "$(jq -r '.frame.revision' <<<"$viewport_workflow_evidence")" ]] || return 1
        [[ "$(jq -r '.frame.image_id' <<<"$viewport_evidence")" != "$(jq -r '.frame.image_id' <<<"$viewport_workflow_evidence")" ]] || return 1
    else
        [[ "$(jq -r '.frame.revision' <<<"$viewport_evidence")" == "$startup_revision" ]] || return 1
        [[ "$(jq -r '.frame.image_id' <<<"$viewport_evidence")" != "$startup_image_id" ]] || return 1
    fi
    [[ "$(jq -r '.camera.yaw_degrees' <<<"$viewport_evidence")" != 0 ]] || return 1
    [[ "$(jq -r '.camera.pitch_degrees' <<<"$viewport_evidence")" == "$(jq -r '.camera.pitch_degrees' <<<"$viewport_startup_evidence")" ]] || return 1
    [[ "$(jq -r '.camera.zoom_percent' <<<"$viewport_evidence")" == "$(jq -r '.camera.zoom_percent' <<<"$viewport_startup_evidence")" ]] || return 1
    [[ "$(jq -r '.camera.pan_x' <<<"$viewport_evidence")" == "$(jq -r '.camera.pan_x' <<<"$viewport_startup_evidence")" ]] || return 1
    [[ "$(jq -r '.camera.pan_y' <<<"$viewport_evidence")" == "$(jq -r '.camera.pan_y' <<<"$viewport_startup_evidence")" ]] || return 1
    local image_id
    image_id="$(jq -r '.frame.image_id' <<<"$viewport_evidence")"
    evidence_wire_ready "$image_id" || return 1
    viewport_orbit_evidence="$viewport_evidence"
    final_image_id="$image_id"
    final_image_id_json="$image_id"
    return 0
}

write_child_wrapper() {
    WRAPPER="${EVIDENCE_ROOT}/child-wrapper.sh"
    TUI_STATUS_FILE="${EVIDENCE_ROOT}/tui-exit-status"
    cat >"$WRAPPER" <<'EOF'
#!/usr/bin/env bash
set -u
script_bin="$1"
tui_binary="$2"
project_root="$3"
pty_output="$4"
pty_input="$5"
tui_stderr="$6"
status_file="$7"
printf -v command_line '%q %q' "$tui_binary" "$project_root"
set +e
"$script_bin" --quiet --flush --log-out="$pty_output" --log-in="$pty_input" --command="$command_line" 2>"$tui_stderr"
status=$?
printf '%s\n' "$status" >"$status_file"
printf '\r\n[graphical-runner] tui-exited status=%s\r\n' "$status"
IFS= read -r -n 1 _ || true
exit "$status"
EOF
    chmod 700 "$WRAPPER"
}

start_compositor() {
    env -u TERM -u TERM_PROGRAM -u TMUX -u SSH_CONNECTION -u SSH_TTY \
        LC_ALL=C.UTF-8 LANG=C.UTF-8 HOME="$EVIDENCE_ROOT/home" \
        XDG_CONFIG_HOME="$EVIDENCE_ROOT/config" XDG_CACHE_HOME="$EVIDENCE_ROOT/cache" \
        XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" WAYLAND_DISPLAY="$WAYLAND_DISPLAY" \
        setsid weston --backend=headless-backend.so --socket="$WAYLAND_DISPLAY" \
        --width="$COMPOSITOR_WIDTH" --height="$COMPOSITOR_HEIGHT" --idle-time=0 \
        --continue-without-input >"$WESTON_LOG" 2>&1 &
    WESTON_PID=$!
    wait_until "$RUNNER_TIMEOUT_SECONDS" wayland_ready || die compositor_unavailable 'Weston did not create the private Wayland socket'
    weston_status='running'
}

find_output() {
    OUTPUT_NAME="$(wlr-randr --json 2>/dev/null | jq -r '.[0].name // empty')"
    [[ -n "$OUTPUT_NAME" ]] || die compositor_output_unavailable 'private compositor output was not discoverable'
}

start_probe_stimulus() {
    : >"$STIMULUS_ERROR"
    setsid env -u TERM -u TERM_PROGRAM -u TMUX -u SSH_CONNECTION -u SSH_TTY \
        LC_ALL=C.UTF-8 LANG=C.UTF-8 WAYLAND_DISPLAY="$WAYLAND_DISPLAY" \
        THREETERM_GRAPHICAL_PROBE_STIMULUS_SECONDS="$PROBE_STIMULUS_SECONDS" \
        THREETERM_GRAPHICAL_OUTPUT="$OUTPUT_NAME" \
        THREETERM_GRAPHICAL_STIMULUS_ERROR="$STIMULUS_ERROR" \
        bash -c '
            fail_stimulus() {
                printf "%s\n" "$1" >"$THREETERM_GRAPHICAL_STIMULUS_ERROR"
                exit 1
            }
            deadline=$((SECONDS + ${THREETERM_GRAPHICAL_PROBE_STIMULUS_SECONDS:-2}))
            while ((SECONDS < deadline)); do
                ydotool mousemove --absolute 100 100 >/dev/null 2>&1 || fail_stimulus "ydotool mousemove 100,100 failed"
                ydotool click 0xC0 >/dev/null 2>&1 || fail_stimulus "ydotool click failed"
                ydotool mousemove --absolute 400 400 >/dev/null 2>&1 || fail_stimulus "ydotool mousemove 400,400 failed"
                ydotool click 0xC0 >/dev/null 2>&1 || fail_stimulus "ydotool click failed"
                wlr-randr --output "$THREETERM_GRAPHICAL_OUTPUT" --custom-mode 640x480 >/dev/null 2>&1 || fail_stimulus "wlr-randr resize to 640x480 failed"
                wlr-randr --output "$THREETERM_GRAPHICAL_OUTPUT" --custom-mode 800x600 >/dev/null 2>&1 || fail_stimulus "wlr-randr restore to 800x600 failed"
                sleep 0.1
            done
        ' &
    STIMULUS_PID=$!
}

check_probe_stimulus() {
    [[ -s "$STIMULUS_ERROR" ]] || return 0
    local detail
    detail="$(tr -d '\r\n' <"$STIMULUS_ERROR" 2>/dev/null || true)"
    [[ -n "$detail" ]] || detail='graphical probe stimulus failed'
    probe_status='failed'
    die probe_stimulus_failed "$detail"
}

wait_for_probe_stimulus() {
    [[ -n "$STIMULUS_PID" ]] || {
        check_probe_stimulus
        return 0
    }
    local stimulus_status=0
    wait_for_pid_exit "$STIMULUS_PID" "$RUNNER_TIMEOUT_SECONDS" ||
        die probe_stimulus_timeout 'graphical probe stimulus did not complete within its bounded window'
    wait "$STIMULUS_PID" 2>/dev/null || stimulus_status=$?
    STIMULUS_PID=''
    check_probe_stimulus
    ((stimulus_status == 0)) || {
        probe_status='failed'
        die probe_stimulus_failed "graphical probe stimulus exited with status ${stimulus_status}"
    }
}

start_ghostty() {
    mkdir -p "$EVIDENCE_ROOT/home" "$EVIDENCE_ROOT/config" "$EVIDENCE_ROOT/cache"
    write_child_wrapper
    env -u TERM -u TERM_PROGRAM -u TMUX -u SSH_CONNECTION -u SSH_TTY \
        LC_ALL=C.UTF-8 LANG=C.UTF-8 HOME="$EVIDENCE_ROOT/home" \
        THREETERM_PALETTE=catppuccin \
        XDG_CONFIG_HOME="$EVIDENCE_ROOT/config" XDG_CACHE_HOME="$EVIDENCE_ROOT/cache" \
        XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" WAYLAND_DISPLAY="$WAYLAND_DISPLAY" \
        setsid ghostty --gtk-single-instance=false --class=ThreeTermGraphicalTest \
        --window-width="$TERMINAL_COLUMNS" --window-height="$TERMINAL_ROWS" \
        --font-size=12 --quit-after-last-window=true -e bash "$WRAPPER" \
        "$(command -v script)" "$TUI_BINARY" "$PROJECT_ROOT" "$PTY_OUTPUT" "$PTY_INPUT" \
        "$TUI_STDERR" "$TUI_STATUS_FILE" &
    GHOSTTY_PID=$!
}

wait_for_tui_readiness() {
    check_probe_stimulus
    wait_until "$RUNNER_TIMEOUT_SECONDS" screen_ready || {
        check_probe_stimulus
        die graphical_capture_failed 'grim could not capture the Ghostty surface at the fixed geometry'
    }
    wait_until "$RUNNER_TIMEOUT_SECONDS" readiness_ready || {
        check_probe_stimulus
        probe_status='failed'
        die capability_or_readiness_failed "${readiness_detail:-production readiness was not observed after the positive capability probe}"
    }
    check_probe_stimulus
    capture_screenshot "$STARTUP_SCREENSHOT" || die startup_screenshot_failed 'startup screenshot was not fixed at 800x600'
    local ocr
    ocr="$(tesseract "$STARTUP_SCREENSHOT" stdout 2>/dev/null || true)"
    grep -Fq 'Interactive Modeling ready' <<<"$ocr" || die visible_readiness_failed 'readiness marker was not visible in the startup screenshot'
    grep -Fq 'Viewport presented' <<<"$ocr" || die viewport_marker_not_visible 'viewport evidence marker was not visible in the startup screenshot'
    if [[ "$TEST_ID" != 'production_tui_create_project_extrude' ]]; then
        rendered_viewport_ready "$STARTUP_SCREENSHOT" || die viewport_not_rendered 'startup screenshot does not contain palette-bound rendered geometry'
    fi
    wait_for_probe_stimulus
    probe_status='passed'
    readiness_status='passed'
}

run_orbit() {
    local before_acks after_acks
    before_acks="$(grep -aFc ';OK' "$PTY_INPUT" 2>/dev/null || true)"
    wtype -k Right || die input_injection_failed 'compositor keyboard input could not send Right'
    wait_until "$RUNNER_TIMEOUT_SECONDS" orbit_ready || die orbit_not_observed 'production orbit acknowledgement was not observed'
    after_acks="$(grep -aFc ';OK' "$PTY_INPUT" 2>/dev/null || true)"
    ((after_acks > before_acks)) || die orbit_not_acknowledged 'orbit did not receive a new Kitty acknowledgement'
    capture_screenshot "$ORBIT_SCREENSHOT" || die orbit_screenshot_failed 'orbit screenshot was not fixed at 800x600'
    local ocr
    ocr="$(tesseract "$ORBIT_SCREENSHOT" stdout 2>/dev/null || true)"
    grep -Fq 'Orbit right' <<<"$ocr" || die orbit_marker_not_visible 'orbit acknowledgement was not visible in the orbit screenshot'
    grep -Fq 'Viewport presented' <<<"$ocr" || die orbit_viewport_marker_not_visible 'orbit viewport evidence was not visible in the orbit screenshot'
    rendered_viewport_ready "$ORBIT_SCREENSHOT" || die orbit_not_rendered 'orbit screenshot does not contain palette-bound rendered geometry'
    [[ "$(jq -r '.frame.image_id' <<<"$viewport_evidence")" != "$startup_image_id" ]] ||
        die orbit_not_rendered 'orbit did not receive a new viewport frame'
    [[ "$(jq -r '.camera.yaw_degrees' <<<"$viewport_evidence")" != 0 ]] ||
        die orbit_not_rendered 'orbit did not change the camera yaw'
    orbit_status='passed'
}

navigation_frame_ready() {
    local expected_feature="$1"
    local expected_yaw="$2"
    local expected_pitch="$3"
    local expected_zoom="$4"
    local expected_pan_x="$5"
    local expected_pan_y="$6"
    local marker="$7"
    local before_acks="$8"
    local before_markers="$9"
    local image_id ack_count marker_count

    extract_viewport_evidence || return 1
    validate_selected_viewport_evidence "$viewport_evidence" "$expected_feature" || return 1
    image_id="$(jq -r '.frame.image_id' <<<"$viewport_evidence")"
    [[ "$image_id" != "$navigation_previous_image_id" ]] || return 1
    [[ "$(jq -r '.frame.revision' <<<"$viewport_evidence")" == "$startup_revision" ]] || return 1
    evidence_wire_ready "$image_id" || return 1
    ack_count="$(grep -aFc ';OK' "$PTY_INPUT" 2>/dev/null || true)"
    marker_count="$(grep -aFc "$marker" "$PTY_OUTPUT" 2>/dev/null || true)"
    ((ack_count > before_acks)) || return 1
    ((marker_count > before_markers)) || return 1
    jq -e \
        --arg feature "$expected_feature" \
        --argjson yaw "$expected_yaw" \
        --argjson pitch "$expected_pitch" \
        --argjson zoom "$expected_zoom" \
        --argjson pan_x "$expected_pan_x" \
        --argjson pan_y "$expected_pan_y" \
        '.selected_feature_id == $feature and
         .camera.yaw_degrees == $yaw and
         .camera.pitch_degrees == $pitch and
         .camera.zoom_percent == $zoom and
         .camera.pan_x == $pan_x and
         .camera.pan_y == $pan_y and
         (.scene.body_pixels + .scene.edge_pixels + .scene.non_background_pixels) > 0' \
        <<<"$viewport_evidence" >/dev/null 2>&1
}

navigation_marker_line() {
    local marker="$1"
    local line
    while IFS= read -r line; do
        [[ "$line" == *"$marker"* ]] || continue
        printf '%s' "$line"
    done < <(tr '\r' '\n' <"$PTY_OUTPUT" 2>/dev/null || true)
}

capture_navigation_frame() {
    local action="$1"
    local input="$2"
    local marker="$3"
    local visible_text="$4"
    local screenshot="$5"
    local crop="$6"
    local ocr
    capture_screenshot "$screenshot" || die "${action}_screenshot_failed" "${action} screenshot was not fixed at 800x600"
    magick "$screenshot" -crop 800x480+0+0 "$crop" ||
        die "${action}_viewport_crop_failed" "${action} viewport crop could not be retained"
    ocr="$(tesseract "$screenshot" stdout 2>/dev/null || true)"
    grep -Fq 'Viewport presented' <<<"$ocr" ||
        die "${action}_viewport_marker_not_visible" "${action} viewport evidence was not visible in the screenshot"
    grep -Fq "$visible_text" <<<"$ocr" ||
        die "${action}_acknowledgement_not_visible" "${action} acknowledgement was not visible in the screenshot"
    local screenshot_sha crop_sha acknowledgement_text
    screenshot_sha="$(sha256sum "$screenshot" | cut -d' ' -f1)"
    crop_sha="$(sha256sum "$crop" | cut -d' ' -f1)"
    if [[ -n "$navigation_previous_viewport_crop_sha256" && "$crop_sha" == "$navigation_previous_viewport_crop_sha256" ]]; then
        die "${action}_presentation_unchanged" "${action} viewport crop matched the previous rendered presentation"
    fi
    acknowledgement_text="$(navigation_marker_line "$marker")"
    jq -n \
        --arg schema_version "$SCHEMA_VERSION" \
        --arg action "$action" \
        --arg input "$input" \
        --arg marker "$marker" \
        --arg acknowledgement_text "$acknowledgement_text" \
        --arg screenshot "$screenshot" \
        --arg screenshot_sha256 "$screenshot_sha" \
        --arg viewport_crop "$crop" \
        --arg viewport_crop_sha256 "$crop_sha" \
        --argjson viewport "$viewport_evidence" \
        '{schema_version:$schema_version,action:$action,input:$input,acknowledgement:{marker:$marker,text:$acknowledgement_text},frame:$viewport.frame,camera:$viewport.camera,selection:$viewport.selected_feature_id,revision:$viewport.frame.revision,viewport_evidence:$viewport,screenshots:{full:{path:$screenshot,sha256:$screenshot_sha256},viewport:{path:$viewport_crop,sha256:$viewport_crop_sha256}}}' \
        >>"$NAVIGATION_TRANSCRIPT"
    navigation_previous_image_id="$(jq -r '.frame.image_id' <<<"$viewport_evidence")"
    navigation_previous_viewport_crop_sha256="$crop_sha"
    final_image_id="$navigation_previous_image_id"
    final_image_id_json="$final_image_id"
}

run_keyboard_navigation() {
    [[ "$TEST_ID" == 'production_tui_keyboard_navigation' ]] || return 0
    : >"$NAVIGATION_TRANSCRIPT"
    magick "$STARTUP_SCREENSHOT" -crop 800x480+0+0 "$STARTUP_VIEWPORT_CROP" ||
        die startup_viewport_crop_failed 'startup viewport crop could not be retained'
    navigation_previous_viewport_crop_sha256="$(sha256sum "$STARTUP_VIEWPORT_CROP" | cut -d' ' -f1)"
    navigation_project_generation_digest_before="$(project_generation_digest "$PROJECT_ROOT")" ||
        die navigation_project_generation_digest_failed 'saved project generation could not be digested before navigation'
    navigation_previous_image_id="$startup_image_id"

    local before_acks before_markers
    before_acks="$(grep -aFc ';OK' "$PTY_INPUT" 2>/dev/null || true)"
    before_markers="$(grep -aFc '[selection-glyph]' "$PTY_OUTPUT" 2>/dev/null || true)"
    wtype -k Down || die input_injection_failed 'compositor keyboard input could not select the feature'
    wait_until "$RUNNER_TIMEOUT_SECONDS" navigation_frame_ready \
        'l-bracket' 0 25 100 0 0 '[selection-glyph]' "$before_acks" "$before_markers" ||
        die selection_not_observed 'keyboard selection did not produce bound acknowledged viewport evidence'
    validate_selected_viewport_evidence "$viewport_evidence" 'l-bracket' ||
        die selection_not_bound 'selection evidence did not name l-bracket'
    capture_navigation_frame 'selection' $'\e[B' '[selection-glyph]' \
        'selected feature l-bracket' \
        "$SELECTION_SCREENSHOT" "$SELECTION_VIEWPORT_CROP"
    viewport_selection_evidence="$viewport_evidence"
    rendered_selected_viewport_ready "$SELECTION_SCREENSHOT" ||
        die selection_not_rendered 'selection screenshot does not contain selected geometry pixels'

    before_acks="$(grep -aFc ';OK' "$PTY_INPUT" 2>/dev/null || true)"
    before_markers="$(grep -aFc '[motion-trail] Orbit right' "$PTY_OUTPUT" 2>/dev/null || true)"
    wtype -k Right || die input_injection_failed 'compositor keyboard input could not orbit the viewport'
    wait_until "$RUNNER_TIMEOUT_SECONDS" navigation_frame_ready \
        'l-bracket' 5 25 100 0 0 '[motion-trail] Orbit right' "$before_acks" "$before_markers" ||
        die orbit_not_observed 'keyboard orbit did not produce updated viewport evidence'
    capture_navigation_frame 'orbit' $'\e[C' '[motion-trail] Orbit right' \
        'Orbit right' \
        "$ORBIT_SCREENSHOT" "$ORBIT_VIEWPORT_CROP"
    viewport_orbit_evidence="$viewport_evidence"
    rendered_selected_viewport_ready "$ORBIT_SCREENSHOT" ||
        die orbit_not_rendered 'orbit screenshot does not contain selected geometry pixels'

    before_acks="$(grep -aFc ';OK' "$PTY_INPUT" 2>/dev/null || true)"
    before_markers="$(grep -aFc '[motion-trail] Pan up' "$PTY_OUTPUT" 2>/dev/null || true)"
    wtype w || die input_injection_failed 'compositor keyboard input could not pan the viewport'
    wait_until "$RUNNER_TIMEOUT_SECONDS" navigation_frame_ready \
        'l-bracket' 5 25 100 0 -5 '[motion-trail] Pan up' "$before_acks" "$before_markers" ||
        die pan_not_observed 'keyboard pan did not produce updated viewport evidence'
    capture_navigation_frame 'pan' 'w' '[motion-trail] Pan up' \
        'Pan up' \
        "$PAN_SCREENSHOT" "$PAN_VIEWPORT_CROP"
    viewport_pan_evidence="$viewport_evidence"
    rendered_selected_viewport_ready "$PAN_SCREENSHOT" ||
        die pan_not_rendered 'pan screenshot does not contain selected geometry pixels'

    before_acks="$(grep -aFc ';OK' "$PTY_INPUT" 2>/dev/null || true)"
    before_markers="$(grep -aFc '[motion-trail] Zoom in' "$PTY_OUTPUT" 2>/dev/null || true)"
    wtype + || die input_injection_failed 'compositor keyboard input could not zoom the viewport'
    wait_until "$RUNNER_TIMEOUT_SECONDS" navigation_frame_ready \
        'l-bracket' 5 25 105 0 -5 '[motion-trail] Zoom in' "$before_acks" "$before_markers" ||
        die zoom_not_observed 'keyboard zoom did not produce updated viewport evidence'
    capture_navigation_frame 'zoom' '+' '[motion-trail] Zoom in' \
        'Zoom in' \
        "$ZOOM_SCREENSHOT" "$ZOOM_VIEWPORT_CROP"
    viewport_zoom_evidence="$viewport_evidence"
    rendered_selected_viewport_ready "$ZOOM_SCREENSHOT" ||
        die zoom_not_rendered 'zoom screenshot does not contain selected geometry pixels'
    navigation_status='passed'
}

wait_for_output_marker() {
    local marker="$1"
    wait_for_output_marker_count "$marker" 1
}

wait_for_output_marker_count() {
    local marker="$1"
    local expected_count="$2"
    wait_until "$RUNNER_TIMEOUT_SECONDS" bash -c \
        '[[ "$(grep -aFc "$1" "$2" 2>/dev/null || true)" -ge "$3" ]]' \
        bash "$marker" "$PTY_OUTPUT" "$expected_count" ||
        die workflow_marker_missing "production TUI did not emit ${marker} ${expected_count} time(s)"
}

create_project_extrude_viewport_ready() {
    extract_viewport_evidence || return 1
    validate_viewport_evidence "$viewport_evidence" || return 1
    [[ "$(jq -r '.scene.solids[0].feature_id' <<<"$viewport_evidence")" == 'keyboard-extrude' ]]
}

run_create_project_extrude() {
    [[ "$TEST_ID" == 'production_tui_create_project_extrude' ]] || return 0
    [[ ! -e "$CREATED_PROJECT_ROOT" ]] ||
        die fresh_destination_exists "fresh workflow destination already exists: $CREATED_PROJECT_ROOT"

    local project_request
    project_request="$(jq -n --arg destination "$CREATED_PROJECT_ROOT" '{destination:$destination}')" ||
        die project_request_failed 'fresh project destination request could not be encoded'
    EXPECTED_FEATURE_ID='keyboard-extrude'

    wtype -M ctrl -k p -m ctrl || die input_injection_failed 'compositor keyboard input could not open the command palette'
    wtype new-project || die input_injection_failed 'compositor keyboard input could not type new-project'
    wtype -k Return || die input_injection_failed 'compositor keyboard input could not select new-project'
    wtype "$project_request" || die input_injection_failed 'compositor keyboard input could not type the project destination'
    wtype -M ctrl -k v -m ctrl || die input_injection_failed 'compositor keyboard input could not request project preview'
    wait_for_output_marker '[dashed-outline] Preview: new-project'
    wtype -k Escape || die input_injection_failed 'compositor keyboard input could not cancel the project draft'
    wait_for_output_marker '[cancellation-glyph] Cancellation: command draft discarded'
    [[ ! -e "$CREATED_PROJECT_ROOT" ]] || die cancellation_mutated_project 'cancelled project draft created a destination'

    wtype -M ctrl -k p -m ctrl || die input_injection_failed 'compositor keyboard input could not reopen the command palette'
    wtype new-project || die input_injection_failed 'compositor keyboard input could not retype new-project'
    wtype -k Return || die input_injection_failed 'compositor keyboard input could not reselect new-project'
    wtype "$project_request" || die input_injection_failed 'compositor keyboard input could not retype the project destination'
    wtype -M ctrl -k v -m ctrl || die input_injection_failed 'compositor keyboard input could not request the second project preview'
    wait_for_output_marker_count '[dashed-outline] Preview: new-project' 2
    wtype -M ctrl -k Return -m ctrl || die input_injection_failed 'compositor keyboard input could not commit the project'
    wait_for_output_marker 'Project created:'
    wait_for_output_marker 'transaction_count=0'
    [[ -f "$CREATED_PROJECT_ROOT/manifest.json" ]] || die project_not_created 'new-project commit did not create a manifest'
    capture_screenshot "$PROJECT_CREATED_SCREENSHOT" || die project_created_screenshot_failed 'project-created screenshot was not fixed at 800x600'
    local project_ocr
    project_ocr="$(tesseract "$PROJECT_CREATED_SCREENSHOT" stdout 2>/dev/null || true)"
    grep -Fq 'Project created' <<<"$project_ocr" || die project_marker_not_visible 'project creation acknowledgement was not visible in the project-created screenshot'
    grep -Fq 'transaction_count=0' <<<"$project_ocr" || die project_transaction_marker_not_visible 'zero-transaction checkpoint was not visible in the project-created screenshot'
    grep -Fq 'Viewport presented' <<<"$project_ocr" || die project_viewport_marker_not_visible 'empty viewport evidence was not visible in the project-created screenshot'
    local project_generation_digest_before_extrusion
    project_generation_digest_before_extrusion="$(project_generation_digest "$CREATED_PROJECT_ROOT")" ||
        die project_generation_digest_failed 'project generation could not be digested before extrusion cancellation'

    local extrude_request
    extrude_request='{"feature_id":"keyboard-extrude","profile":[[0,0],[10,0],[10,5],[0,5]],"height":3,"mode":"additive"}'
    wtype -M ctrl -k p -m ctrl || die input_injection_failed 'compositor keyboard input could not open the extrusion palette'
    wtype extrude || die input_injection_failed 'compositor keyboard input could not type extrude'
    wtype -k Return || die input_injection_failed 'compositor keyboard input could not select extrude'
    wtype "$extrude_request" || die input_injection_failed 'compositor keyboard input could not type the extrusion request'
    wtype -M ctrl -k v -m ctrl || die input_injection_failed 'compositor keyboard input could not request extrusion preview'
    wait_for_output_marker '[dashed-outline] Preview: extrude'
    wtype -k Escape || die input_injection_failed 'compositor keyboard input could not cancel the extrusion draft'
    wait_for_output_marker_count '[cancellation-glyph] Cancellation: command draft discarded' 2
    [[ "$(project_generation_digest "$CREATED_PROJECT_ROOT")" == "$project_generation_digest_before_extrusion" ]] ||
        die cancellation_mutated_project 'cancelled extrusion draft changed the created project state'
    [[ ! -e "$PROJECT_ROOT" && -f "$CREATED_PROJECT_ROOT/manifest.json" ]] ||
        die cancellation_changed_routing 'cancelled extrusion draft changed the active project routing'
    [[ ! -f "$CREATED_PROJECT_ROOT/brep/keyboard-extrude.brep" ]] ||
        die cancellation_mutated_project 'cancelled extrusion draft created a BREP'

    wtype -M ctrl -k p -m ctrl || die input_injection_failed 'compositor keyboard input could not reopen the extrusion palette'
    wtype extrude || die input_injection_failed 'compositor keyboard input could not retype extrude'
    wtype -k Return || die input_injection_failed 'compositor keyboard input could not reselect extrude'
    wtype "$extrude_request" || die input_injection_failed 'compositor keyboard input could not retype the extrusion request'
    wtype -M ctrl -k v -m ctrl || die input_injection_failed 'compositor keyboard input could not request the second extrusion preview'
    wait_for_output_marker_count '[dashed-outline] Preview: extrude' 2
    wtype -M ctrl -k Return -m ctrl || die input_injection_failed 'compositor keyboard input could not commit the extrusion'
    wait_for_output_marker '[selection-glyph] Commit: extrude'
    wtype -k Down || die input_injection_failed 'compositor keyboard input could not select the committed extrusion'
    wait_for_output_marker 'selected feature keyboard-extrude'
    wait_until "$RUNNER_TIMEOUT_SECONDS" create_project_extrude_viewport_ready ||
        die workflow_viewport_invalid "fresh extrusion viewport evidence was invalid: $viewport_evidence"
    local brep_path="$CREATED_PROJECT_ROOT/brep/keyboard-extrude.brep"
    [[ -f "$brep_path" ]] || die project_brep_missing 'fresh extrusion did not create the committed BREP'
    local brep_sha256
    brep_sha256="$(sha256sum "$brep_path" | cut -d' ' -f1)"
    jq -s -e --arg bundle_path "$CREATED_PROJECT_ROOT" \
        --arg brep_path "$brep_path" \
        --arg brep_sha256 "$brep_sha256" \
        --arg feature_id 'keyboard-extrude' \
        --argjson profile '[[0,0],[10,0],[10,5],[0,5]]' \
        --argjson height 3 \
        --arg mode 'additive' \
        '.[0] as $manifest | (.[1:] | map(select(.feature_id == $feature_id and .intent.command == "extrude")) | last) as $entry | select(($manifest.generation_id | type == "string" and length > 0) and ($manifest.revision_id | type == "string" and length > 0) and ($manifest.revision_hash | type == "string" and length > 0) and ($manifest.transaction_count | type == "number" and . == 1) and ($entry.intent.deterministic_inputs.profile == $profile) and ($entry.intent.deterministic_inputs.height == $height) and ($entry.intent.mode == $mode)) | {project_identity:{bundle_path:$bundle_path,generation_id:$manifest.generation_id,revision_id:$manifest.revision_id,revision_hash:$manifest.revision_hash,transaction_count:$manifest.transaction_count},intent:{feature_id:$entry.feature_id,profile:$entry.intent.deterministic_inputs.profile,height:$entry.intent.deterministic_inputs.height,mode:$entry.intent.mode},derived_result:{brep_path:$brep_path,brep_sha256:$brep_sha256}}' \
        "$CREATED_PROJECT_ROOT/manifest.json" "$CREATED_PROJECT_ROOT/transactions.log" >"$PROJECT_IDENTITY" ||
        die project_identity_failed 'project identity evidence could not be written'
    capture_screenshot "$EXTRUSION_COMMITTED_SCREENSHOT" || die extrusion_screenshot_failed 'extrusion-committed screenshot was not fixed at 800x600'
    local ocr
    ocr="$(tesseract "$EXTRUSION_COMMITTED_SCREENSHOT" stdout 2>/dev/null || true)"
    grep -Fq 'Project created' <<<"$ocr" || die project_marker_not_visible 'project creation acknowledgement was not visible in the extrusion screenshot'
    grep -Fq 'keyboard-extrude' <<<"$ocr" || die extrusion_marker_not_visible 'extrusion acknowledgement was not visible in the extrusion screenshot'
    grep -Fq 'Viewport presented' <<<"$ocr" || die workflow_viewport_marker_not_visible 'final viewport evidence was not visible in the extrusion screenshot'
    rendered_viewport_ready "$EXTRUSION_COMMITTED_SCREENSHOT" || die workflow_not_rendered 'fresh extrusion screenshot does not contain palette-bound rendered geometry'
    local image_id
    image_id="$(jq -r '.frame.image_id' <<<"$viewport_evidence")"
    evidence_wire_ready "$image_id" || die workflow_not_acknowledged 'final fresh workflow frame did not receive a Kitty acknowledgement'
    viewport_workflow_evidence="$viewport_evidence"
    workflow_status='passed'
}

reinforcing_frame_ready() {
    extract_viewport_evidence || return 1
    validate_viewport_evidence "$viewport_evidence" || return 1
    local image_id
    image_id="$(jq -r '.frame.image_id' <<<"$viewport_evidence")"
    [[ "$image_id" != "$final_image_id" ]] || return 1
    evidence_wire_ready "$image_id" || return 1
    final_image_id="$image_id"
    final_image_id_json="$image_id"
}

run_reinforcing_features() {
    [[ "$TEST_ID" == 'production_tui_mirror_pattern_reinforcing_features' ]] || return 0
    : >"$REINFORCING_TRANSCRIPT"

    local mirror_request='{"feature_id":"tui-mirror-pad","base_feature_id":"reinforce-pad","plane_point":[0,0,0],"plane_normal":[1,-1,0]}'
    local linear_request='{"feature_id":"tui-linear-pads","base_feature_id":"reinforce-pad","direction":[1,0,0],"count":3,"spacing":12}'
    local circular_request='{"feature_id":"tui-circular-lugs","base_feature_id":"reinforce-pad","axis_point":[30,15,0],"axis_normal":[0,0,1],"angle_step":1.5707963267948966,"count":4}'

    wtype -M ctrl -k p -m ctrl || die input_injection_failed 'compositor keyboard input could not open the mirror palette'
    wtype mirror || die input_injection_failed 'compositor keyboard input could not type mirror'
    wtype -k Return || die input_injection_failed 'compositor keyboard input could not select mirror'
    wtype "$mirror_request" || die input_injection_failed 'compositor keyboard input could not type the mirror request'
    wtype -M ctrl -k v -m ctrl || die input_injection_failed 'compositor keyboard input could not request the mirror preview'
    wait_for_output_marker '[dashed-outline] Preview: mirror'
    wtype -k Escape || die input_injection_failed 'compositor keyboard input could not cancel the mirror draft'
    wait_for_output_marker '[cancellation-glyph] Cancellation: command draft discarded'
    jq -n \
        --arg command mirror \
        --argjson request "$mirror_request" \
        '{command:$command,request:$request,preview_marker:"[dashed-outline] Preview: mirror",cancellation_marker:"[cancellation-glyph] Cancellation: command draft discarded",outcome:"cancelled"}' \
        >>"$REINFORCING_TRANSCRIPT"
    reinforcing_commit() {
        local command="$1"
        local request="$2"
        local screenshot="$3"
        local marker="[selection-glyph] Commit: ${command}"
        local ocr screenshot_sha
        case "$command" in
            mirror) reinforcing_viewport_phase=3 ;;
            linear-pattern) reinforcing_viewport_phase=4 ;;
            circular-pattern) reinforcing_viewport_phase=5 ;;
        esac
        wtype -M ctrl -k p -m ctrl || die input_injection_failed "compositor keyboard input could not open the ${command} palette"
        wtype "$command" || die input_injection_failed "compositor keyboard input could not type ${command}"
        wtype -k Return || die input_injection_failed "compositor keyboard input could not select ${command}"
        wtype "$request" || die input_injection_failed "compositor keyboard input could not type the ${command} request"
        wtype -M ctrl -k v -m ctrl || die input_injection_failed "compositor keyboard input could not request the ${command} preview"
        wait_for_output_marker "[dashed-outline] Preview: ${command}"
        wtype -M ctrl -k Return -m ctrl || die input_injection_failed "compositor keyboard input could not commit ${command}"
        wait_for_output_marker "$marker"
        wait_until "$RUNNER_TIMEOUT_SECONDS" reinforcing_frame_ready ||
            die reinforcing_viewport_invalid "${command} did not produce valid five-solid viewport evidence"
        capture_screenshot "$screenshot" || die reinforcing_screenshot_failed "${command} screenshot was not fixed at 800x600"
        ocr="$(tesseract "$screenshot" stdout 2>/dev/null || true)"
        grep -Fq 'Viewport presented' <<<"$ocr" || die reinforcing_viewport_marker_not_visible "${command} viewport evidence was not visible in the screenshot"
        grep -Fq "$command" <<<"$ocr" || die reinforcing_marker_not_visible "${command} acknowledgement was not visible in the screenshot"
        rendered_viewport_ready "$screenshot" || die reinforcing_not_rendered "${command} screenshot does not contain palette-bound rendered geometry"
        screenshot_sha="$(sha256sum "$screenshot" | cut -d' ' -f1)"
        jq -n \
            --arg command "$command" \
            --argjson request "$request" \
            --arg marker "$marker" \
            --arg screenshot "$screenshot" \
            --arg screenshot_sha256 "$screenshot_sha" \
            --argjson viewport "$viewport_evidence" \
            '{command:$command,request:$request,preview_marker:("[dashed-outline] Preview: " + $command),commit_marker:$marker,response:{feature_id:("tui-" + (if $command == "linear-pattern" then "linear-pads" elif $command == "circular-pattern" then "circular-lugs" else "mirror-pad" end)),revision:$viewport.frame.revision},feature_ids:($viewport.scene.solids | map(.feature_id)),frame:$viewport.frame,viewport_evidence:$viewport,screenshot:{path:$screenshot,sha256:$screenshot_sha256}}' \
            >>"$REINFORCING_TRANSCRIPT"
    }

    reinforcing_commit mirror "$mirror_request" "$MIRROR_SCREENSHOT"
    reinforcing_commit linear-pattern "$linear_request" "$LINEAR_PATTERN_SCREENSHOT"
    reinforcing_commit circular-pattern "$circular_request" "$CIRCULAR_PATTERN_SCREENSHOT"
    viewport_workflow_evidence="$viewport_evidence"
    workflow_status='passed'
}

read_tui_status() {
    [[ -s "$TUI_STATUS_FILE" ]] || return 1
    local status
    status="$(tr -d '[:space:]' <"$TUI_STATUS_FILE")"
    [[ "$status" == 0 ]]
}

release_child_wrapper() {
    wtype -k Return || return 1
    return 0
}

verify_cleanup() {
    local line image_id
    cleanup_deletions='[]'
    final_delete_image_id=''
    final_delete_image_id_json='null'
    while IFS= read -r line; do
        if [[ "$line" =~ a=d,d=I,i=([0-9]+) ]]; then
            image_id="${BASH_REMATCH[1]}"
            cleanup_deletions="$(jq --argjson image_id "$image_id" '. + [$image_id]' <<<"$cleanup_deletions")"
            final_delete_image_id="$image_id"
            final_delete_image_id_json="$image_id"
        fi
    done < <(tr '\r' '\n' <"$PTY_OUTPUT" 2>/dev/null || true)
    [[ -n "$final_delete_image_id" ]] || die image_cleanup_missing 'production output did not delete the active Kitty image'
    [[ "$final_delete_image_id" == "$final_image_id" ]] || die image_cleanup_wrong_image 'production output did not delete the final acknowledged Kitty image'
    for marker in '?1049l' '?1002l' '?1004l' '?1006l' '?1016l' '?2026l'; do
        grep -aFq "$marker" "$PTY_OUTPUT" 2>/dev/null || die terminal_cleanup_missing "production output did not contain cleanup marker $marker"
    done
    grep -aFq '?25h' "$PTY_OUTPUT" 2>/dev/null || die terminal_cleanup_missing 'production output did not restore the cursor'
    grep -aFq '[0m' "$PTY_OUTPUT" 2>/dev/null || die terminal_cleanup_missing 'production output did not reset terminal attributes'
    cleanup_status='passed'
}

cleanup_screenshot_clear() {
    local body_pixels edge_pixels selected_body selected_edge
    body_pixels="$(rgb_pixel_count "$CLEANUP_SCREENSHOT" 125 125 152)" || return 1
    edge_pixels="$(rgb_pixel_count "$CLEANUP_SCREENSHOT" 198 165 162)" || return 1
    selected_body="$(rgb_pixel_count "$CLEANUP_SCREENSHOT" 192 193 222)" || return 1
    selected_edge="$(rgb_pixel_count "$CLEANUP_SCREENSHOT" 121 111 136)" || return 1
    [[ "$body_pixels" == 0 && "$edge_pixels" == 0 && "$selected_body" == 0 && "$selected_edge" == 0 ]]
}

redact_transient_source_revision() {
    [[ "$TEST_ID" == 'production_tui_create_project_extrude' ]] || return 0
    local path
    for path in "$PTY_OUTPUT" "$TUI_STDERR" "$WESTON_LOG"; do
        [[ -f "$path" ]] || continue
        sed -i "s/${TRANSIENT_EMPTY_PROJECT_SOURCE_REVISION}/${PERSISTED_TRANSIENT_SOURCE_REVISION}/g" "$path" || return 1
    done
    failure_detail="${failure_detail//${TRANSIENT_EMPTY_PROJECT_SOURCE_REVISION}/$PERSISTED_TRANSIENT_SOURCE_REVISION}"
}

wait_for_pid_exit() {
    local pid="$1"
    local seconds="$2"
    local deadline=$((SECONDS + seconds))
    while ((SECONDS < deadline)); do
        kill -0 "$pid" >/dev/null 2>&1 || return 0
        sleep 0.1
    done
    return 1
}

terminate_group() {
    local pid="$1"
    [[ -n "$pid" ]] || return 0
    if kill -0 "$pid" >/dev/null 2>&1; then
        kill -- -"$pid" >/dev/null 2>&1 || kill -TERM "$pid" >/dev/null 2>&1 || true
        if ! wait_for_pid_exit "$pid" 3; then
            kill -KILL -- -"$pid" >/dev/null 2>&1 || kill -KILL "$pid" >/dev/null 2>&1 || true
        fi
    fi
}

write_manifest() {
    local final_status="$1"
    local artifacts='[]'
    local kind path bytes digest record
    local -a evidence_files=(
        "$PTY_OUTPUT" "$PTY_INPUT" "$TUI_STDERR" "$WESTON_LOG" "$TOOL_VERSIONS"
        "$STARTUP_SCREENSHOT" "$PROJECT_CREATED_SCREENSHOT" "$EXTRUSION_COMMITTED_SCREENSHOT"
        "$ORBIT_SCREENSHOT" "$SELECTION_SCREENSHOT" "$PAN_SCREENSHOT" "$ZOOM_SCREENSHOT"
        "$STARTUP_VIEWPORT_CROP" "$SELECTION_VIEWPORT_CROP" "$ORBIT_VIEWPORT_CROP"
        "$PAN_VIEWPORT_CROP" "$ZOOM_VIEWPORT_CROP" "$NAVIGATION_TRANSCRIPT"
        "$MIRROR_SCREENSHOT" "$LINEAR_PATTERN_SCREENSHOT" "$CIRCULAR_PATTERN_SCREENSHOT"
        "$REINFORCING_TRANSCRIPT"
        "$CLEANUP_SCREENSHOT" "$FAILURE_SCREENSHOT"
        "$DIFF_LOG" "${EVIDENCE_ROOT}/window-ready.png" "$STIMULUS_ERROR"
    )
    local -a evidence_kinds=(
        pty_output pty_input tui_stderr compositor_log tool_versions
        startup_screenshot project_created_screenshot extrusion_committed_screenshot
        orbit_screenshot selection_screenshot pan_screenshot zoom_screenshot
        startup_viewport_crop selection_viewport_crop orbit_viewport_crop
        pan_viewport_crop zoom_viewport_crop navigation_transcript
        mirror_screenshot linear_pattern_screenshot circular_pattern_screenshot
        reinforcing_transcript
        cleanup_screenshot failure_screenshot
        orbit_difference window_screenshot probe_stimulus_error
    )
    if [[ "$TEST_ID" == 'production_tui_create_project_extrude' ]]; then
        evidence_files+=(
            "$PROJECT_IDENTITY"
            "$CREATED_PROJECT_ROOT/manifest.json"
            "$CREATED_PROJECT_ROOT/brep/keyboard-extrude.brep"
        )
        evidence_kinds+=(project_identity created_project_manifest derived_brep)
    fi
    if command -v jq >/dev/null 2>&1 && command -v sha256sum >/dev/null 2>&1; then
        local index
        for index in "${!evidence_files[@]}"; do
            path="${evidence_files[$index]}"
            [[ -f "$path" ]] || continue
            kind="${evidence_kinds[$index]}"
            bytes="$(wc -c <"$path" | tr -d '[:space:]')"
            digest="$(sha256sum "$path" | cut -d' ' -f1)"
            record="$(jq -n --arg kind "$kind" --arg path "$path" --argjson bytes "$bytes" --arg sha256 "$digest" '{kind:$kind,path:$path,bytes:$bytes,sha256:$sha256}')"
            artifacts="$(jq --argjson record "$record" '. + [$record]' <<<"$artifacts")"
        done
        local result='failed'
        [[ "$success" == 1 && "$final_status" == 0 ]] && result='passed'
        local dirty_json="$source_dirty"
        local failure_json
        local tool_versions_json='{}'
        if [[ -f "$TOOL_VERSIONS" ]]; then
            tool_versions_json="$(<"$TOOL_VERSIONS")"
        fi
        if [[ -n "$failure_code" ]]; then
            failure_json="$(jq -n --arg code "$failure_code" --arg detail "$failure_detail" '{code:$code,detail:$detail}')"
        else
            failure_json='null'
        fi
        local persisted_viewport_startup="$viewport_startup_evidence"
        if [[ "$TEST_ID" == 'production_tui_create_project_extrude' ]]; then
            persisted_viewport_startup="$(jq \
                --arg expected "$TRANSIENT_EMPTY_PROJECT_SOURCE_REVISION" \
                --arg replacement "$PERSISTED_TRANSIENT_SOURCE_REVISION" \
                'if . == null then null elif .frame.revision == $expected then .frame.revision = $replacement else . end' \
                <<<"$viewport_startup_evidence")"
        fi
        jq -n \
            --arg schema_version "$SCHEMA_VERSION" \
            --arg result "$result" \
            --arg test "$TEST_ID" \
            --arg source_commit "$source_commit" \
            --argjson source_dirty "$dirty_json" \
            --arg locale "$LOCALE" \
            --argjson compositor_width "$COMPOSITOR_WIDTH" \
            --argjson compositor_height "$COMPOSITOR_HEIGHT" \
            --argjson terminal_columns "$TERMINAL_COLUMNS" \
            --argjson terminal_rows "$TERMINAL_ROWS" \
            --arg viewport_crop "$VIEWPORT_CROP" \
            --arg palette "catppuccin" \
            --arg toolchain_contract "$TOOLCHAIN_CONTRACT" \
            --argjson tool_versions "$tool_versions_json" \
            --arg probe_status "$probe_status" \
            --arg readiness_status "$readiness_status" \
            --arg orbit_status "$orbit_status" \
            --arg cleanup_status "$cleanup_status" \
            --arg tui_status "$tui_status" \
            --arg ghostty_status "$ghostty_status" \
            --arg weston_status "$weston_status" \
            --arg owned_processes_status "$owned_processes_status" \
             --argjson viewport_startup "$persisted_viewport_startup" \
             --argjson viewport_orbit "$viewport_orbit_evidence" \
             --argjson viewport_workflow "$viewport_workflow_evidence" \
             --argjson viewport_selection "$viewport_selection_evidence" \
             --argjson viewport_pan "$viewport_pan_evidence" \
             --argjson viewport_zoom "$viewport_zoom_evidence" \
             --argjson final_image_id "$final_image_id_json" \
            --argjson final_delete_image_id "$final_delete_image_id_json" \
             --argjson cleanup_deletions "$cleanup_deletions" \
             --arg navigation_transcript "$NAVIGATION_TRANSCRIPT" \
              --arg navigation_before "$navigation_project_generation_digest_before" \
              --arg navigation_after "$navigation_project_generation_digest_after" \
             --arg navigation_status "$navigation_status" \
             --arg workflow_status "$workflow_status" \
            --argjson artifacts "$artifacts" \
            --argjson failure "$failure_json" \
              '{schema_version:$schema_version,result:$result,test:$test,source:{commit:$source_commit,dirty:$source_dirty},configuration:{locale:$locale,palette:$palette,compositor:{width:$compositor_width,height:$compositor_height},terminal:{columns:$terminal_columns,rows:$terminal_rows},viewport_crop:$viewport_crop},toolchain:{contract:$toolchain_contract,versions:$tool_versions},events:{probe:$probe_status,readiness:$readiness_status,orbit:$orbit_status,navigation:$navigation_status,workflow:$workflow_status,cleanup:$cleanup_status},processes:{tui:$tui_status,ghostty:$ghostty_status,weston:$weston_status,owned:$owned_processes_status},viewport:{startup:$viewport_startup,selection:$viewport_selection,orbit:$viewport_orbit,pan:$viewport_pan,zoom:$viewport_zoom,workflow:$viewport_workflow},navigation:{transcript:$navigation_transcript,project_generation_digest_before:$navigation_before,project_generation_digest_after:$navigation_after},cleanup_evidence:{final_image_id:$final_image_id,final_delete_image_id:$final_delete_image_id,deletions:$cleanup_deletions},failure:$failure,artifacts:$artifacts}' \
            >"${MANIFEST}.tmp.$$" && mv -f "${MANIFEST}.tmp.$$" "$MANIFEST"
    else
        write_minimal_manifest
    fi
}

on_exit() {
    local final_status=$?
    trap - EXIT INT TERM
    set +e
    if [[ "$success" != 1 && -n "$GHOSTTY_PID" && -f "$(command -v grim 2>/dev/null)" ]]; then
        capture_screenshot "$FAILURE_SCREENSHOT" || true
    fi
    [[ -n "$STIMULUS_PID" ]] && terminate_group "$STIMULUS_PID"
    [[ -n "$GHOSTTY_PID" ]] && terminate_group "$GHOSTTY_PID"
    [[ -n "$WESTON_PID" ]] && terminate_group "$WESTON_PID"
    if [[ "$success" == 1 && "$final_status" == 0 ]]; then
        owned_processes_status='stopped'
    else
        owned_processes_status='terminated'
    fi
    if [[ -z "$failure_code" && "$final_status" != 0 ]]; then
        failure_code='runner_failed'
        failure_detail='graphical runner exited before completing its evidence contract'
    fi
    if ! redact_transient_source_revision; then
        success=0
        final_status=1
        failure_code='transient_identity_redaction_failed'
        failure_detail='transient source revision could not be removed from retained evidence'
    fi
    write_manifest "$final_status"
    exit "$final_status"
}

trap on_exit EXIT INT TERM

[[ -z "$path_failure_code" ]] || die "$path_failure_code" "$path_failure_detail"
[[ -x "$TUI_BINARY" ]] || die tui_binary_unavailable "TUI binary is not executable: $TUI_BINARY"
mkdir -p "$XDG_RUNTIME_DIR" || die runtime_root_unavailable 'private XDG_RUNTIME_DIR could not be created'
chmod 700 "$XDG_RUNTIME_DIR" || die runtime_root_unavailable 'private XDG_RUNTIME_DIR could not be secured'
check_prerequisites
if [[ "${THREETERM_GRAPHICAL_FORCE_CAPABILITY_DENIAL:-0}" == 1 ]]; then
    die capability_or_readiness_failed 'capability denial fixture requested'
fi
start_compositor
find_output
start_ghostty
start_probe_stimulus
wait_for_tui_readiness
if [[ "$TEST_ID" == 'production_tui_create_project_extrude' ]]; then
    run_create_project_extrude
fi
if [[ "$TEST_ID" == 'production_tui_mirror_pattern_reinforcing_features' ]]; then
    run_reinforcing_features
fi
if [[ "$TEST_ID" == 'production_tui_keyboard_navigation' ]]; then
    run_keyboard_navigation
else
    run_orbit
fi
wtype q || die input_injection_failed 'compositor keyboard input could not send q'
wait_until "$RUNNER_TIMEOUT_SECONDS" read_tui_status || die tui_exit_timeout 'production TUI did not exit cleanly after q'
tui_status='passed'
if [[ "$TEST_ID" == 'production_tui_keyboard_navigation' ]]; then
    navigation_project_generation_digest_after="$(project_generation_digest "$PROJECT_ROOT")" ||
        die navigation_project_generation_digest_failed 'saved project generation could not be digested after navigation'
    [[ "$navigation_project_generation_digest_after" == "$navigation_project_generation_digest_before" ]] ||
        die navigation_mutated_project 'keyboard navigation changed the canonical saved project'
fi
verify_cleanup
capture_screenshot "$CLEANUP_SCREENSHOT" || die cleanup_screenshot_failed 'cleanup screenshot was not captured before Ghostty teardown'
cleanup_screenshot_clear || die cleanup_screenshot_not_clear 'cleanup screenshot still contains viewport geometry pixels'
cleanup_screenshot_taken=1
release_child_wrapper || die wrapper_release_failed 'Ghostty child wrapper did not accept its release event'
wait_for_pid_exit "$GHOSTTY_PID" "$RUNNER_TIMEOUT_SECONDS" || die ghostty_exit_timeout 'Ghostty did not exit after the production TUI wrapper was released'
ghostty_status='stopped'
wait "$GHOSTTY_PID" >/dev/null 2>&1 || true
terminate_group "$STIMULUS_PID"
STIMULUS_PID=''
terminate_group "$WESTON_PID"
WESTON_PID=''
weston_status='stopped'
success=1
exit 0
