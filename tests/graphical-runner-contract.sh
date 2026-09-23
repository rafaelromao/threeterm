#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNNER="${ROOT}/.github/scripts/graphical-tui.sh"

test -f "${RUNNER}"
bash -n "${RUNNER}"

help="$(bash "${RUNNER}" --help)"
for expected in \
    'production_tui_ghostty_session' \
    'production_tui_create_project_extrude' \
    'production_tui_keyboard_navigation' \
    'production_tui_save_reopen_validate_export' \
    'production_tui_mirror_pattern_reinforcing_features' \
    'production_tui_tapered_lofted_reinforcements' \
    '--tui-binary' \
    '--project-root' \
    '--evidence-root' \
    '--validate-viewport-evidence' \
    '--print-plan'; do
    grep -Fq -- "${expected}" <<<"${help}"
done

plan="$(bash "${RUNNER}" --print-plan)"
jq -e '
    .schema_version == "threeterm.graphical-tui/1" and
    .result == "not_run" and
    .test == "production_tui_ghostty_session" and
    .configuration.locale == "C.UTF-8" and
    .configuration.palette == "catppuccin" and
    .configuration.compositor.width == 800 and
    .configuration.compositor.height == 600 and
    .configuration.terminal.columns == 80 and
    .configuration.terminal.rows == 24 and
    .configuration.viewport_crop == "0,0,800x480" and
    (.requirements | index("weston")) and
    (.requirements | index("ghostty")) and
    (.requirements | index("wtype")) and
    (.requirements | index("ydotool")) and
    (.requirements | index("wlr-randr")) and
    (.requirements | index("grim")) and
    (.requirements | index("tesseract")) and
    (.requirements | index("magick")) and
    (.requirements | index("jq")) and
    (.requirements | index("sha256sum")) and
    (.requirements | index("script")) and
    (.requirements | index("setsid")) and
    (.requirements | index("timeout"))
' <<<"${plan}" >/dev/null

fresh_plan="$(bash "${RUNNER}" production_tui_create_project_extrude --print-plan)"
jq -e '
    .schema_version == "threeterm.graphical-tui.create-project-extrude/1" and
    .result == "not_run" and
    .test == "production_tui_create_project_extrude"
' <<<"${fresh_plan}" >/dev/null

navigation_plan="$(bash "${RUNNER}" production_tui_keyboard_navigation --print-plan)"
jq -e '
    .schema_version == "threeterm.graphical-tui.keyboard-navigation/1" and
    .result == "not_run" and
    .test == "production_tui_keyboard_navigation"
' <<<"${navigation_plan}" >/dev/null

lifecycle_plan="$(bash "${RUNNER}" production_tui_save_reopen_validate_export --print-plan)"
jq -e '
    .schema_version == "threeterm.graphical-tui.save-reopen-validate-export/1" and
    .result == "not_run" and
    .test == "production_tui_save_reopen_validate_export"
' <<<"${lifecycle_plan}" >/dev/null
reinforcing_plan="$(bash "${RUNNER}" production_tui_mirror_pattern_reinforcing_features --print-plan)"
jq -e '
    .schema_version == "threeterm.graphical-tui.mirror-pattern-reinforcing-features/1" and
    .result == "not_run" and
    .test == "production_tui_mirror_pattern_reinforcing_features"
' <<<"${reinforcing_plan}" >/dev/null
reinforcement_plan="$(bash "${RUNNER}" production_tui_tapered_lofted_reinforcements --print-plan)"
jq -e '
    .schema_version == "threeterm.graphical-tui.tapered-lofted-reinforcements/1" and
    .result == "not_run" and
    .test == "production_tui_tapered_lofted_reinforcements"
' <<<"${reinforcement_plan}" >/dev/null

for required in \
    'LC_ALL=C.UTF-8' \
    'LANG=C.UTF-8' \
    '-u TERM' \
    '-u TERM_PROGRAM' \
    '-u TMUX' \
    '-u SSH_CONNECTION' \
    '-u SSH_TTY' \
    '--log-out' \
    '--log-in' \
    'setsid' \
    'kill -- -' \
    'threeterm.graphical-tui/1' \
    'threeterm.graphical-tui.create-project-extrude/1' \
    'threeterm.graphical-tui.keyboard-navigation/1' \
    'threeterm.graphical-tui.save-reopen-validate-export/1' \
    'threeterm.graphical-tui.mirror-pattern-reinforcing-features/1' \
    'threeterm.graphical-tui.tapered-lofted-reinforcements/1' \
    'empty-startup.png' \
    'project-created.png' \
    'extrusion-committed.png' \
    'project-identity.json' \
    'occt_worker_unavailable' \
    'capability_or_readiness_failed' \
    'prerequisite_missing' \
    'toolchain_contract_missing' \
    'THREETERM_GRAPHICAL_FORCE_CAPABILITY_DENIAL' \
    'selected feature keyboard-extrude' \
    'selected_feature_id' \
    'navigation-transcript.jsonl' \
    'selection.png' \
    'pan.png' \
    'zoom.png' \
    'selection-viewport.png' \
    'pan-viewport.png' \
    'zoom-viewport.png' \
    'startup-viewport.png' \
    'failure.png' \
    'navigation_project_generation_digest' \
    'rendered_selected_viewport_ready' \
    'reinforcing_viewport_phase' \
    'reinforcing_frame_ready' \
    'reinforcing_transcript' \
    'mirror_screenshot' \
    'linear_pattern_screenshot' \
    'circular_pattern_screenshot' \
    'empty-session-source' \
    'project_generation_digest' \
    'cancellation_changed_routing' \
    'brep_sha256' \
    'transaction_count' \
    'intent' \
    'final_image_id' \
    'final_delete_image_id' \
    'created_project_manifest' \
    'derived_brep' \
    'validate_reinforcement_viewport_evidence' \
    'tapered-reinforcement' \
    'lofted-gusset' \
    'tapered-committed.png' \
    'lofted-committed.png' \
    'reinforcement-transcript.jsonl' \
    'expected_revision' \
    'bundle_path' \
    '1.3.1-arch2' \
    'THREETERM_PALETTE=catppuccin' \
    'threeterm.viewport-evidence/1' \
    'Viewport presented' \
    'validate_viewport_evidence' \
    'rgb_pixel_count' \
    'l-bracket' \
    'sha256' \
    'probe_stimulus_failed' \
    'probe_stimulus_timeout' \
    'probe-stimulus-error' \
    'fail_stimulus' \
    'check_probe_stimulus' \
    'wait_for_probe_stimulus' \
    '800x480' \
    '?1002l' \
    'cleanup_evidence'; do
    grep -Fq -- "${required}" "${RUNNER}"
done

for lifecycle_required in \
    'production_tui_save_reopen_validate_export' \
    'relaunching-tui' \
    'save.png' \
    'reopen.png' \
    'validation.png' \
    'export.png' \
    'stl-integrity.json' \
    'Save completed' \
    'Load completed' \
    '[validation-status] Validation passed' \
    '[export-status] Export completed' \
    'export_destination_exists' \
    'project_fixture_missing' \
    'stl_integrity_preflight_failed' \
    'evidence_root_cleanup_failed' \
    'LIFECYCLE_REVISION' \
    'threeterm-stl-integrity' \
    'timeout --kill-after=5s' \
    'tui-export/l-bracket.stl' \
    'threeterm_host::stl_integrity::verify_path' \
    '--append'; do
    grep -Fq -- "${lifecycle_required}" "${RUNNER}"
done

if grep -Fq 'magick compare' "${RUNNER}"; then
    echo "graphical navigation must not gate on full-screen pixel equality" >&2
    exit 1
fi

readiness_body="$(sed -n '/^wait_for_tui_readiness()/,/^}/p' "${RUNNER}")"
grep -Fq 'wait_for_probe_stimulus' <<<"${readiness_body}" || {
    echo "wait_for_tui_readiness must wait for probe stimulus completion before passing" >&2
    exit 1
}
for marker in 'startup_screenshot' 'rendered_viewport_ready' 'wait_for_probe_stimulus'; do
    grep -Fq "${marker}" <<<"${readiness_body}" || {
        echo "wait_for_tui_readiness is missing required gate ${marker}" >&2
        exit 1
    }
done

navigation_body="$(sed -n '/^navigation_frame_ready()/,/^}/p' "${RUNNER}")"
grep -Fq 'startup_revision' <<<"${navigation_body}" || {
    echo "navigation frames must remain bound to the startup project revision" >&2
    exit 1
}
grep -Fq 'evidence_wire_ready "$image_id"' <<<"${navigation_body}" || {
    echo "navigation frames must verify the acknowledgement for their exact image" >&2
    exit 1
}
python3 - "${RUNNER}" <<'PY'
import re, sys
path = sys.argv[1]
body = open(path).read()
match = re.search(r'^wait_for_tui_readiness\(\) \{(.*?)^\}', body, re.M | re.S)
if not match:
    print("wait_for_tui_readiness not found", file=sys.stderr)
    sys.exit(1)
block = match.group(1)
for marker in ("capture_screenshot", "rendered_viewport_ready", "wait_for_probe_stimulus", "probe_status='passed'"):
    if marker not in block:
        print(f"readiness gate missing {marker}", file=sys.stderr)
        sys.exit(1)
positions = [block.index(marker) for marker in ("rendered_viewport_ready", "wait_for_probe_stimulus", "probe_status='passed'")]
if positions != sorted(positions):
    print("probe stimulus completion must gate the passed status after viewport rendering", file=sys.stderr)
    sys.exit(1)
PY

if grep -Eq 'ydotool .* \|\| true' "${RUNNER}"; then
    echo "ydotool stimulus must fail closed instead of ignoring failures" >&2
    exit 1
fi
if grep -Eq 'wlr-randr .* \|\| true' "${RUNNER}"; then
    echo "wlr-randr stimulus must fail closed instead of ignoring failures" >&2
    exit 1
fi

evidence="$(mktemp -d)"
trap 'rm -rf "${evidence}"' EXIT
set +e
THREETERM_GRAPHICAL_TOOLCHAIN_CONTRACT="${evidence}/missing-contract" \
    bash "${RUNNER}" production_tui_ghostty_session \
    --tui-binary /bin/true --project-root "${ROOT}" --evidence-root "${evidence}/run" \
    >"${evidence}/stdout" 2>"${evidence}/stderr"
status=$?
set -e
((status == 1))
jq -e '
    .schema_version == "threeterm.graphical-tui/1" and
    .result == "failed" and
    (.failure.code | type == "string")
' "${evidence}/run/manifest.json" >/dev/null

run_invalid_path() {
    local expected_code="$1"
    shift
    local run_root="${evidence}/${expected_code}"
    set +e
    bash "${RUNNER}" production_tui_ghostty_session "$@" \
        --evidence-root "${run_root}" >"${run_root}.stdout" 2>"${run_root}.stderr"
    status=$?
    set -e
    ((status != 0))
    jq -e --arg expected "$expected_code" '.result == "failed" and .failure.code == $expected' \
        "${run_root}/manifest.json" >/dev/null
}

run_invalid_path project_root_unavailable \
    --tui-binary /bin/true --project-root "${evidence}/missing-project"
run_invalid_path tui_binary_unavailable \
    --tui-binary "${evidence}/missing-bin/tui" --project-root "${ROOT}"

valid_viewport_evidence="${evidence}/valid-viewport-evidence.json"
jq -n '
    {
        schema_version: "threeterm.viewport-evidence/1",
        acknowledgement: "viewport-presented",
        frame: {frame_token: 1, image_id: 1, generation: 0, revision: "revision", width: 800, height: 480},
        scene: {
            solids: [{feature_id: "l-bracket", triangle_count: 12}],
            triangle_count: 12,
            body_pixels: 100,
            edge_pixels: 10,
            non_background_pixels: 110
        },
        palette: {
            name: "catppuccin",
            colors: {
                background: [29,29,45], body: [125,125,152], edge: [198,165,162],
                grid: [119,155,149], selected_body: [192,193,222], selected_edge: [121,111,136],
                candidate_body: [164,153,179], candidate_edge: [221,207,180],
                drag_feedback: [146,167,189], overlay: [155,132,152],
                warning: [235,220,193], error: [208,174,171]
            }
        },
        camera: {yaw_degrees: 0, pitch_degrees: 20, zoom_percent: 100, pan_x: 0, pan_y: 0}
    }
' >"${valid_viewport_evidence}"
bash "${RUNNER}" --validate-viewport-evidence "${valid_viewport_evidence}"
jq -n '{schema_version: "threeterm.viewport-evidence/1"}' >"${evidence}/invalid-viewport-evidence.json"
if bash "${RUNNER}" --validate-viewport-evidence "${evidence}/invalid-viewport-evidence.json"; then
    echo "malformed viewport evidence unexpectedly passed" >&2
    exit 1
fi

tool_versions_dir="$(mktemp -d "${evidence}/tool-versions.XXXXXX")"
fake_bin="${tool_versions_dir}/bin"
mkdir -p "${fake_bin}"
fake_version() {
    local tool="$1"
    local version="$2"
    printf '#!/usr/bin/env bash\nprintf "%%s\\n" "%s"\n' "${version}" >"${fake_bin}/${tool}"
    chmod +x "${fake_bin}/${tool}"
}
fake_version wtype 'wtype 0.4-test'
fake_version ydotool 'ydotool 1.0-test'
fake_version wlr-randr 'wlr-randr 0.3-test'
fake_version grim 'grim 1.4-test'
fake_version tesseract 'tesseract 5.0-test'
fake_version magick 'ImageMagick 7.1-test'
fake_version ghostty 'ghostty 1.3.1-arch2-test'
cat >"${fake_bin}/weston" <<'EOF'
#!/usr/bin/env bash
if [[ "${1:-}" == "--version" ]]; then
    printf '%s\n' 'weston 12.0-test'
else
    sleep 30
fi
EOF
chmod +x "${fake_bin}/weston"
contract="${tool_versions_dir}/toolchain.env"
{
    printf 'WESTON_VERSION=%s\n' 'weston 12.0-test'
    printf 'GHOSTTY_VERSION=%s\n' '1.3.1-arch2'
    printf 'WTYPE_VERSION=%s\n' 'wtype 0.4-test'
    printf 'YDTOOL_VERSION=%s\n' 'ydotool 1.0-test'
    printf 'WLR_RANDR_VERSION=%s\n' 'wlr-randr 0.3-test'
    printf 'GRIM_VERSION=%s\n' 'grim 1.4-test'
    printf 'TESSERACT_VERSION=%s\n' 'tesseract 5.0-test'
    printf 'MAGICK_VERSION=%s\n' 'ImageMagick 7.1-test'
    printf 'JQ_VERSION=%s\n' "$(jq --version 2>&1)"
    printf 'SHA256SUM_VERSION=%s\n' "$(sha256sum --version 2>&1 | head -1)"
    printf 'SCRIPT_VERSION=%s\n' "$(script --version 2>&1 | head -1)"
    printf 'SETSID_VERSION=%s\n' "$(setsid --version 2>&1 | head -1)"
    printf 'TIMEOUT_VERSION=%s\n' "$(timeout --version 2>&1 | head -1)"
    printf 'WESTON_HEADLESS_BACKEND=%s\n' 'headless-backend.so'
} >"${contract}"
expected_hash="$(sha256sum "${contract}" | cut -d' ' -f1)"
run_root="${tool_versions_dir}/run"
set +e
PATH="${fake_bin}:${PATH}" THREETERM_GRAPHICAL_TOOLCHAIN_CONTRACT="${contract}" \
    THREETERM_GRAPHICAL_TIMEOUT_SECONDS=2 \
    bash "${RUNNER}" production_tui_ghostty_session \
    --tui-binary /bin/true --project-root "${ROOT}" --evidence-root "${run_root}" \
    >"${tool_versions_dir}/stdout" 2>"${tool_versions_dir}/stderr"
status=$?
set -e
((status != 0))
jq -e --arg expected_hash "${expected_hash}" '
    .contract_sha256 == $expected_hash and
    (.outputs.weston | contains("weston 12.0-test")) and
    (.outputs.ghostty | contains("1.3.1-arch2"))
' "${run_root}/tool-versions.json" >/dev/null
jq -e '.result == "failed" and .failure.code == "compositor_unavailable"' \
    "${run_root}/manifest.json" >/dev/null

fresh_worker_run_root="${tool_versions_dir}/fresh-worker-run"
set +e
PATH="${fake_bin}:${PATH}" THREETERM_GRAPHICAL_TOOLCHAIN_CONTRACT="${contract}" \
    THREETERM_OCCTBUILD_WORKER="${tool_versions_dir}/missing-worker" \
    THREETERM_GRAPHICAL_TIMEOUT_SECONDS=2 \
    bash "${RUNNER}" production_tui_create_project_extrude \
    --tui-binary /bin/true --project-root "${tool_versions_dir}/fresh-launch" \
    --evidence-root "${fresh_worker_run_root}" \
    >"${tool_versions_dir}/fresh-worker-stdout" 2>"${tool_versions_dir}/fresh-worker-stderr"
fresh_worker_status=$?
set -e
((fresh_worker_status == 1))
jq -e '
    .schema_version == "threeterm.graphical-tui.create-project-extrude/1" and
    .result == "failed" and .failure.code == "occt_worker_unavailable"
' "${fresh_worker_run_root}/manifest.json" >/dev/null

missing_toolchain_run_root="${tool_versions_dir}/missing-toolchain-run"
set +e
PATH="${fake_bin}:${PATH}" THREETERM_GRAPHICAL_TOOLCHAIN_CONTRACT="${evidence}/missing-toolchain-contract" \
    bash "${RUNNER}" production_tui_create_project_extrude \
    --tui-binary /bin/true --project-root "${tool_versions_dir}/missing-toolchain-launch" \
    --evidence-root "${missing_toolchain_run_root}" \
    >"${tool_versions_dir}/missing-toolchain-stdout" 2>"${tool_versions_dir}/missing-toolchain-stderr"
missing_toolchain_status=$?
set -e
((missing_toolchain_status == 1))
jq -e '
    .schema_version == "threeterm.graphical-tui.create-project-extrude/1" and
    .result == "failed" and .failure.code == "toolchain_contract_missing"
' "${missing_toolchain_run_root}/manifest.json" >/dev/null

capability_run_root="${tool_versions_dir}/capability-run"
set +e
PATH="${fake_bin}:${PATH}" THREETERM_GRAPHICAL_TOOLCHAIN_CONTRACT="${contract}" \
    THREETERM_OCCTBUILD_WORKER=/bin/true THREETERM_GRAPHICAL_FORCE_CAPABILITY_DENIAL=1 \
    bash "${RUNNER}" production_tui_create_project_extrude \
    --tui-binary /bin/true --project-root "${tool_versions_dir}/capability-launch" \
    --evidence-root "${capability_run_root}" \
    >"${tool_versions_dir}/capability-stdout" 2>"${tool_versions_dir}/capability-stderr"
capability_status=$?
set -e
((capability_status == 1))
jq -e '
    .schema_version == "threeterm.graphical-tui.create-project-extrude/1" and
    .result == "failed" and .failure.code == "capability_or_readiness_failed"
' "${capability_run_root}/manifest.json" >/dev/null

printf '%s\n' 'graphical runner contract satisfied'
