#!/usr/bin/env bash
set -euo pipefail

SCHEMA_VERSION='threeterm.graphical-tui/1'
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
probe_status='not_run'
readiness_status='not_run'
orbit_status='not_run'
cleanup_status='not_run'
ghostty_status='not_run'
tui_status='not_run'
weston_status='not_run'
owned_processes_status='not_run'
cleanup_screenshot_taken=0
source_commit='unknown'
source_dirty=false
path_failure_code=''
path_failure_detail=''

usage() {
    cat <<'EOF'
Usage:
  graphical-tui.sh production_tui_ghostty_session --tui-binary PATH --project-root PATH --evidence-root PATH
  graphical-tui.sh --print-plan

The named test requires a qualified direct-Ghostty graphical environment. It
never spoofs TERM or TERM_PROGRAM and never passes when prerequisites are absent.
EOF
}

print_plan() {
    cat <<'EOF'
{
  "schema_version": "threeterm.graphical-tui/1",
  "result": "not_run",
  "test": "production_tui_ghostty_session",
  "configuration": {
    "locale": "C.UTF-8",
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
        production_tui_ghostty_session)
            [[ -z "${TEST_NAME:-}" ]] || { usage >&2; exit 2; }
            TEST_NAME="$1"
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

[[ "${TEST_NAME:-}" == "$TEST_ID" ]] || { usage >&2; exit 2; }
[[ -n "$TUI_BINARY" && -n "$PROJECT_ROOT" && -n "$EVIDENCE_ROOT" ]] || {
    usage >&2
    exit 2
}

mkdir -p "$EVIDENCE_ROOT" || {
    printf '%s\n' '{"schema_version":"threeterm.graphical-tui/1","result":"failed","integrity":"unavailable","failure":{"code":"evidence_root_unavailable"}}' >&2
    exit 1
}
EVIDENCE_ROOT="$(cd "$EVIDENCE_ROOT" && pwd)"
if PROJECT_ROOT="$(cd "$PROJECT_ROOT" 2>/dev/null && pwd)"; then
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
STARTUP_SCREENSHOT="${EVIDENCE_ROOT}/startup.png"
ORBIT_SCREENSHOT="${EVIDENCE_ROOT}/orbit.png"
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

    local contract_hash='null'
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
        --argjson contract_sha256 "$contract_hash" \
        --argjson outputs "$json" \
        '{contract: $contract, contract_sha256: $contract_sha256, outputs: $outputs}' \
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
    return 0
}

rendered_viewport_ready() {
    local colors
    colors="$(magick "$STARTUP_SCREENSHOT" -crop 800x480+0+0 -format '%k' info: 2>/dev/null || true)"
    [[ "$colors" =~ ^[0-9]+$ && "$colors" -gt 8 ]]
}

orbit_ready() {
    grep -aFq '[motion-trail] Orbit right' "$PTY_OUTPUT" 2>/dev/null || return 1
    grep -aFq 'a=T,t=d' "$PTY_OUTPUT" 2>/dev/null || return 1
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
    rendered_viewport_ready || die viewport_not_rendered 'startup screenshot does not contain a non-flat rendered viewport'
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
    magick "$STARTUP_SCREENSHOT" -crop 800x480+0+0 "${EVIDENCE_ROOT}/startup-viewport.png"
    magick "$ORBIT_SCREENSHOT" -crop 800x480+0+0 "${EVIDENCE_ROOT}/orbit-viewport.png"
    magick compare -metric AE "${EVIDENCE_ROOT}/startup-viewport.png" \
        "${EVIDENCE_ROOT}/orbit-viewport.png" null: 2>"$DIFF_LOG" || true
    local difference
    difference="$(tr -d '[:space:]' <"$DIFF_LOG" 2>/dev/null || true)"
    [[ "$difference" =~ ^[0-9]+$ && "$difference" -gt 0 ]] || die orbit_not_rendered 'orbit did not change the fixed viewport crop'
    orbit_status='passed'
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
    grep -aFq 'a=d,d=I,i=' "$PTY_OUTPUT" 2>/dev/null || die image_cleanup_missing 'production output did not delete the active Kitty image'
    for marker in '?1049l' '?1004l' '?1006l' '?1016l' '?2026l'; do
        grep -aFq "$marker" "$PTY_OUTPUT" 2>/dev/null || die terminal_cleanup_missing "production output did not contain cleanup marker $marker"
    done
    cleanup_status='passed'
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
        "$STARTUP_SCREENSHOT" "$ORBIT_SCREENSHOT" "$CLEANUP_SCREENSHOT" "$FAILURE_SCREENSHOT"
        "$DIFF_LOG" "${EVIDENCE_ROOT}/window-ready.png" "$STIMULUS_ERROR"
    )
    local -a evidence_kinds=(
        pty_output pty_input tui_stderr compositor_log tool_versions
        startup_screenshot orbit_screenshot cleanup_screenshot failure_screenshot
        orbit_difference window_screenshot probe_stimulus_error
    )
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
            --argjson artifacts "$artifacts" \
            --argjson failure "$failure_json" \
            '{schema_version:$schema_version,result:$result,test:$test,source:{commit:$source_commit,dirty:$source_dirty},configuration:{locale:$locale,compositor:{width:$compositor_width,height:$compositor_height},terminal:{columns:$terminal_columns,rows:$terminal_rows},viewport_crop:$viewport_crop},toolchain:{contract:$toolchain_contract,versions:$tool_versions},events:{probe:$probe_status,readiness:$readiness_status,orbit:$orbit_status,cleanup:$cleanup_status},processes:{tui:$tui_status,ghostty:$ghostty_status,weston:$weston_status,owned:$owned_processes_status},failure:$failure,artifacts:$artifacts}' \
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
    write_manifest "$final_status"
    exit "$final_status"
}

trap on_exit EXIT INT TERM

[[ -z "$path_failure_code" ]] || die "$path_failure_code" "$path_failure_detail"
[[ -x "$TUI_BINARY" ]] || die tui_binary_unavailable "TUI binary is not executable: $TUI_BINARY"
mkdir -p "$XDG_RUNTIME_DIR" || die runtime_root_unavailable 'private XDG_RUNTIME_DIR could not be created'
chmod 700 "$XDG_RUNTIME_DIR" || die runtime_root_unavailable 'private XDG_RUNTIME_DIR could not be secured'
check_prerequisites
start_compositor
find_output
start_ghostty
start_probe_stimulus
wait_for_tui_readiness
run_orbit
wtype q || die input_injection_failed 'compositor keyboard input could not send q'
wait_until "$RUNNER_TIMEOUT_SECONDS" read_tui_status || die tui_exit_timeout 'production TUI did not exit cleanly after q'
tui_status='passed'
verify_cleanup
capture_screenshot "$CLEANUP_SCREENSHOT" || die cleanup_screenshot_failed 'cleanup screenshot was not captured before Ghostty teardown'
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
