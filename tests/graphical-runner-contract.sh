#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNNER="${ROOT}/.github/scripts/graphical-tui.sh"

test -f "${RUNNER}"
bash -n "${RUNNER}"

help="$(bash "${RUNNER}" --help)"
for expected in \
    'production_tui_ghostty_session' \
    '--tui-binary' \
    '--project-root' \
    '--evidence-root' \
    '--print-plan'; do
    grep -Fq -- "${expected}" <<<"${help}"
done

plan="$(bash "${RUNNER}" --print-plan)"
jq -e '
    .schema_version == "threeterm.graphical-tui/1" and
    .result == "not_run" and
    .test == "production_tui_ghostty_session" and
    .configuration.locale == "C.UTF-8" and
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
    '1.3.1-arch2' \
    'sha256' \
    '800x480'; do
    grep -Fq -- "${required}" "${RUNNER}"
done

evidence="$(mktemp -d)"
trap 'rm -rf "${evidence}"' EXIT
set +e
THREETERM_GRAPHICAL_TOOLCHAIN_CONTRACT="${evidence}/missing-contract" \
    bash "${RUNNER}" production_tui_ghostty_session \
    --tui-binary /bin/true --project-root "${ROOT}" --evidence-root "${evidence}/run" \
    >"${evidence}/stdout" 2>"${evidence}/stderr"
status=$?
set -e
((status != 0))
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

printf '%s\n' 'graphical runner contract satisfied'
