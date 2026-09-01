#!/usr/bin/env bash
# Conservative, machine-readable performance-claim release gate.

set -euo pipefail

performance_gate_fail() {
    printf 'performance gate: %s\n' "$1" >&2
    return 1
}

performance_gate_block() {
    local record="$1"
    awk '/<!-- PERFORMANCE-RECORD:START -->/{inside=1; next} /<!-- PERFORMANCE-RECORD:END -->/{inside=0} inside{print}' "$record"
}

performance_gate_claim_language() {
    grep -Eiq 'performance|latency|throughput|startup|memory|benchmark|faster|slower|speedup|(^|[^[:alnum:]])[0-9]+([.,][0-9]+)?[[:space:]]*(x|%|ms|millisecond|milliseconds|us|microsecond|microseconds|KB|MB|GB|req/s|ops/s)([^[:alnum:]]|$)|(^|[^[:alnum:]])p(50|95|99)([^[:alnum:]]|$)'
}

performance_gate_claim_lines() {
    grep -E '^ThreeTerm performance claim: ' || true
}

performance_gate_field() {
    local key="$1"
    local line="$2"
    if [[ "$line" =~ (^|[[:space:]])${key}=([^[:space:]]+) ]]; then
        printf '%s\n' "${BASH_REMATCH[2]}"
        return 0
    fi
    return 1
}

performance_gate_has_symlink_component() {
    local root="$1"
    local path="$2"
    local component cursor="$root"
    IFS='/' read -ra components <<<"$path"
    for component in "${components[@]}"; do
        [[ -z "$component" || "$component" == . ]] && continue
        cursor="${cursor}/${component}"
        [[ ! -L "$cursor" ]] || return 0
    done
    return 1
}

verify_performance_material() {
    local root="$1"
    local material="$2"
    local record="$3"
    local expected_commit="$4"
    local expected_tag="$5"
    [[ -z "$material" ]] && return 0
    [[ -f "$material" ]] || { performance_gate_fail "release material is missing: ${material}"; return 1; }
    if ! performance_gate_claim_language <"$material"; then
        return 0
    fi
    [[ -f "$record" ]] || { performance_gate_fail 'performance claim requires a current six-gate record'; return 1; }

    local block
    block="$(performance_gate_block "$record")"
    [[ -n "$block" ]] || { performance_gate_fail 'six-gate record block is missing'; return 1; }
    grep -Fxq 'record_status: SIGNED' <<<"$block" \
        || { performance_gate_fail 'six-gate record is not signed'; return 1; }
    local record_commit
    record_commit="$(grep -E '^release_commit: ' <<<"$block" | cut -d' ' -f2-)"
    [[ "$record_commit" =~ ^[0-9a-f]{40}$ ]] \
        || { performance_gate_fail 'six-gate record commit identity is invalid'; return 1; }
    git -C "$root" merge-base --is-ancestor "$record_commit" "$expected_commit" \
        || { performance_gate_fail 'six-gate record is not bound to the release source history'; return 1; }
    grep -Fxq "release_tag: ${expected_tag}" <<<"$block" \
        || { performance_gate_fail 'six-gate record is bound to a different tag'; return 1; }
    local today
    today="$(date -u +%F)"
    grep -Eq "^record_date: ${today}$" <<<"$block" \
        || { performance_gate_fail 'six-gate record is not current'; return 1; }

    local evidence evidence_digest actual owner signature gate
    evidence="$(grep -E '^evidence_path: ' <<<"$block" | cut -d' ' -f2-)"
    evidence_digest="$(grep -E '^evidence_sha256: ' <<<"$block" | cut -d' ' -f2-)"
    [[ -n "$evidence" && "$evidence" != /* && "$evidence" != *..* && -f "$root/$evidence" && ! -L "$root/$evidence" ]] \
        || { performance_gate_fail 'six-gate evidence path is missing or unsafe'; return 1; }
    if performance_gate_has_symlink_component "$root" "$evidence"; then
        performance_gate_fail 'six-gate evidence path contains a symlink'
        return 1
    fi
    [[ "$evidence" == docs/research/rehearsal-evidence/* ]] \
        || { performance_gate_fail 'six-gate evidence must be checked-in rehearsal evidence'; return 1; }
    git -C "$root" ls-files --error-unmatch -- "$evidence" >/dev/null 2>&1 \
        || { performance_gate_fail 'six-gate evidence is not tracked in the source commit'; return 1; }
    local committed_evidence_digest
    committed_evidence_digest="$(git -C "$root" show "HEAD:${evidence}" | sha256sum | cut -d' ' -f1)"
    [[ "$committed_evidence_digest" == "$evidence_digest" ]] \
        || { performance_gate_fail 'six-gate evidence digest does not match the committed source'; return 1; }
    [[ -n "$(grep -E '^hardware_profile: ' <<<"$block" | cut -d' ' -f2-)" && \
        "$(grep -E '^hardware_profile: ' <<<"$block" | cut -d' ' -f2-)" != 'not recorded' && \
        -n "$(grep -E '^project_scale: ' <<<"$block" | cut -d' ' -f2-)" && \
        "$(grep -E '^project_scale: ' <<<"$block" | cut -d' ' -f2-)" != 'not recorded' ]] \
        || { performance_gate_fail 'six-gate hardware profile or project scale is missing'; return 1; }
    local limitations limitations_digest committed_limitations_digest
    limitations="$(grep -E '^limitations_path: ' <<<"$block" | cut -d' ' -f2-)"
    limitations_digest="$(grep -E '^limitations_sha256: ' <<<"$block" | cut -d' ' -f2-)"
    [[ "$limitations" == docs/release/* && -f "$root/$limitations" && ! -L "$root/$limitations" ]] \
        || { performance_gate_fail 'six-gate limitations document is missing or unsafe'; return 1; }
    git -C "$root" ls-files --error-unmatch -- "$limitations" >/dev/null 2>&1 \
        || { performance_gate_fail 'six-gate limitations document is not tracked'; return 1; }
    committed_limitations_digest="$(git -C "$root" show "HEAD:${limitations}" | sha256sum | cut -d' ' -f1)"
    [[ "$limitations_digest" =~ ^[0-9a-f]{64}$ && "$committed_limitations_digest" == "$limitations_digest" ]] \
        || { performance_gate_fail 'six-gate limitations digest does not match the committed source'; return 1; }
    for limitation in fixture renderer terminal 'project-scale' 'warm and cold' 'input-to-photon' 'human-usability'; do
        grep -Fqi "$limitation" "$root/$limitations" \
            || { performance_gate_fail "limitations document omits ${limitation} scope"; return 1; }
    done
    [[ "$(grep -E '^stl_deterministic: ' <<<"$block" | cut -d' ' -f2-)" == YES && \
        "$(grep -E '^stl_rc1_sha256: ' <<<"$block" | cut -d' ' -f2-)" =~ ^[0-9a-f]{64}$ && \
        "$(grep -E '^stl_rc2_sha256: ' <<<"$block" | cut -d' ' -f2-)" =~ ^[0-9a-f]{64}$ && \
        -n "$(grep -E '^step_comparison: ' <<<"$block" | cut -d' ' -f2-)" && \
        "$(grep -E '^step_comparison: ' <<<"$block" | cut -d' ' -f2-)" != documented && \
        -n "$(grep -E '^three_mf_comparison: ' <<<"$block" | cut -d' ' -f2-)" && \
        "$(grep -E '^three_mf_comparison: ' <<<"$block" | cut -d' ' -f2-)" != documented ]] \
        || { performance_gate_fail 'six-gate two-release comparison evidence is incomplete'; return 1; }
    for field in hardware_cpu hardware_threads hardware_memory_mb hardware_kernel hardware_microcode \
        hardware_container hardware_container_digest hardware_package_versions hardware_toolchain hardware_ghostty \
        hardware_term hardware_term_program hardware_topology fixture_name feature_count transaction_count derived_result_count \
        statistical_method units sample_minimum; do
        value="$(grep -E "^${field}: " <<<"$block" | cut -d' ' -f2-)"
        [[ -n "$value" && "$value" != 'not recorded' ]] \
            || { performance_gate_fail "six-gate field is missing: ${field}"; return 1; }
    done
    [[ "$(grep -E '^hardware_threads: ' <<<"$block" | cut -d' ' -f2-)" =~ ^[1-9][0-9]*$ && \
        "$(grep -E '^hardware_memory_mb: ' <<<"$block" | cut -d' ' -f2-)" =~ ^[1-9][0-9]*$ && \
        "$(grep -E '^feature_count: ' <<<"$block" | cut -d' ' -f2-)" =~ ^[0-9]+$ && \
        "$(grep -E '^transaction_count: ' <<<"$block" | cut -d' ' -f2-)" =~ ^[0-9]+$ && \
        "$(grep -E '^derived_result_count: ' <<<"$block" | cut -d' ' -f2-)" =~ ^[0-9]+$ && \
        "$(grep -E '^sample_minimum: ' <<<"$block" | cut -d' ' -f2-)" =~ ^[3-9][0-9]$|^[1-9][0-9]{2,}$ ]] \
        || { performance_gate_fail 'six-gate profile, scale, or sample fields are invalid'; return 1; }
    [[ "$(grep -E '^statistical_method: ' <<<"$block" | cut -d' ' -f2-)" != 'not-recorded' && \
        "$(grep -E '^units: ' <<<"$block" | cut -d' ' -f2-)" != 'not-recorded' ]] \
        || { performance_gate_fail 'six-gate statistical method is incomplete'; return 1; }
    for field in step_comparison step_comparison_explanation step_claim_impact \
        three_mf_comparison three_mf_comparison_explanation three_mf_claim_impact; do
        value="$(grep -E "^${field}: " <<<"$block" | cut -d' ' -f2-)"
        [[ -n "$value" && "$value" != 'documented' && "$value" != 'not recorded' ]] \
            || { performance_gate_fail "six-gate comparison field is incomplete: ${field}"; return 1; }
    done
    jq -e '.fixture == "l-bracket" and .run_count == 2 and
        (.release_candidates | sort) == ["rc-1", "rc-2"] and
        (.runs | length) == 2 and (.comparisons | length) > 0 and
        (.runs | all(.artifacts | length > 0))' "$root/$evidence" >/dev/null \
        || { performance_gate_fail 'six-gate rehearsal evidence catalog is incomplete'; return 1; }
    local evidence_file evidence_digest_record
    while IFS=$'\t' read -r evidence_file evidence_digest_record; do
        [[ "$evidence_file" != /* && "$evidence_file" != *..* && -f "$root/docs/research/rehearsal-evidence/l-bracket/$evidence_file" ]] \
            || { performance_gate_fail 'six-gate catalog contains an unsafe artifact path'; return 1; }
        [[ "$(sha256sum "$root/docs/research/rehearsal-evidence/l-bracket/$evidence_file" | cut -d' ' -f1)" == "$evidence_digest_record" ]] \
            || { performance_gate_fail "six-gate catalog digest mismatch: ${evidence_file}"; return 1; }
    done < <(jq -r '.runs[].artifacts[] | [.relative_path, .sha256] | @tsv' "$root/$evidence")
    local stl_rc1 stl_rc2 recorded_stl_rc1 recorded_stl_rc2
    stl_rc1="$(jq -er '.runs[] | select(.release_candidate == "rc-1") | .artifacts[] | select(.relative_path | endswith("/export/l-bracket.stl")) | .sha256' "$root/$evidence")"
    stl_rc2="$(jq -er '.runs[] | select(.release_candidate == "rc-2") | .artifacts[] | select(.relative_path | endswith("/export/l-bracket.stl")) | .sha256' "$root/$evidence")"
    recorded_stl_rc1="$(grep -E '^stl_rc1_sha256: ' <<<"$block" | cut -d' ' -f2-)"
    recorded_stl_rc2="$(grep -E '^stl_rc2_sha256: ' <<<"$block" | cut -d' ' -f2-)"
    [[ "$stl_rc1" == "$recorded_stl_rc1" && "$stl_rc2" == "$recorded_stl_rc2" ]] \
        || { performance_gate_fail 'six-gate STL hashes do not match the evidence catalog'; return 1; }
    local comparison_class comparison_match
    while IFS= read -r comparison_class; do
        comparison_match=0
        while IFS= read -r comparison; do
            if [[ "$comparison" == "comparison: class=${comparison_class} same_order=YES" ]]; then
                comparison_match=1
                break
            fi
        done < <(grep -E '^comparison: ' <<<"$block")
        (( comparison_match )) \
            || { performance_gate_fail "six-gate comparison is missing for ${comparison_class}"; return 1; }
    done < <(jq -r '.comparisons[].class' "$root/$evidence")
    local resolved_root resolved_evidence
    resolved_root="$(realpath -e "$root")"
    resolved_evidence="$(realpath -e "$root/$evidence")"
    [[ "$resolved_evidence" == "$resolved_root"/* ]] \
        || { performance_gate_fail 'six-gate evidence resolves outside the release source root'; return 1; }
    [[ "$evidence_digest" =~ ^[0-9a-f]{64}$ ]] \
        || { performance_gate_fail 'six-gate evidence digest is invalid'; return 1; }
    actual="$(sha256sum "$root/$evidence" | cut -d' ' -f1)"
    [[ "$actual" == "$evidence_digest" ]] \
        || { performance_gate_fail 'six-gate evidence digest does not match'; return 1; }
    owner="$(grep -E '^owner: ' <<<"$block" | cut -d' ' -f2-)"
    signature="$(grep -E '^record_signature: ' <<<"$block" | cut -d' ' -f2-)"
    [[ -n "$owner" && "$owner" != 'not recorded' && "$owner" != 'unknown' && \
        "$signature" != 'not recorded' && "$signature" != 'unknown' && "$owner" == "$signature" ]] \
        || { performance_gate_fail 'six-gate record signature does not identify the owner'; return 1; }
    for gate in 1 2 3 4 5 6; do
        grep -Fxq "gate_${gate}: PASS" <<<"$block" \
            || { performance_gate_fail "six-gate ${gate} did not pass"; return 1; }
        grep -Fxq "gate_${gate}_signature: ${owner}" <<<"$block" \
            || { performance_gate_fail "six-gate ${gate} is not signed by the owner"; return 1; }
        grep -Fxq "gate_${gate}_date: ${today}" <<<"$block" \
            || { performance_gate_fail "six-gate ${gate} is not dated"; return 1; }
        grep -Fxq "gate_${gate}_signature: not recorded" <<<"$block" \
            && { performance_gate_fail "six-gate ${gate} has no owner signature"; return 1; }
    done

    local claims claim id metric unit percentile fixture scale row field
    claims="$(performance_gate_claim_lines <"$material")"
    [[ -n "$claims" ]] || { performance_gate_fail 'performance claim must use the structured claim grammar'; return 1; }
    if performance_gate_claim_language < <(grep -Ev '^ThreeTerm performance claim: ' "$material"); then
        performance_gate_fail 'performance material contains an unstructured claim'
        return 1
    fi
    while IFS= read -r claim; do
        [[ "$claim" =~ ^ThreeTerm[[:space:]]performance[[:space:]]claim:[[:space:]]id=[A-Za-z0-9._/-]+[[:space:]]metric=[A-Za-z0-9._/-]+[[:space:]]unit=[A-Za-z0-9%/_-]+[[:space:]]percentile=p(50|95|99)[[:space:]]fixture=[A-Za-z0-9._/-]+[[:space:]]scale=[A-Za-z0-9._/-]+$ ]] \
            || { performance_gate_fail 'performance claim has invalid grammar'; return 1; }
        for field in id metric unit percentile fixture scale; do
            performance_gate_field "$field" "${claim#*: }" >/dev/null \
                || { performance_gate_fail "performance claim is missing ${field}"; return 1; }
        done
        id="$(performance_gate_field id "${claim#*: }")"
        metric="$(performance_gate_field metric "${claim#*: }")"
        unit="$(performance_gate_field unit "${claim#*: }")"
        percentile="$(performance_gate_field percentile "${claim#*: }")"
        fixture="$(performance_gate_field fixture "${claim#*: }")"
        scale="$(performance_gate_field scale "${claim#*: }")"
        record_fixture="$(grep -E '^fixture_name: ' <<<"$block" | cut -d' ' -f2-)"
        record_scale="$(grep -E '^project_scale: ' <<<"$block" | cut -d' ' -f2-)"
        [[ "${fixture,,}" == "${record_fixture,,}" && "$scale" == "$record_scale" ]] \
            || { performance_gate_fail "performance claim is outside the signed fixture or scale: ${id}"; return 1; }
        row_prefix="claim: id=${id} metric=${metric} unit=${unit} percentile=${percentile} fixture=${fixture} scale=${scale} "
        claim_admitted=0
        while IFS= read -r row; do
            [[ "${row#"$row_prefix"}" != "$row" ]] || continue
            if [[ "${row#"$row_prefix"}" =~ ^n_rc1=([3-9][0-9]|[1-9][0-9]{2,})[[:space:]]n_rc2=([3-9][0-9]|[1-9][0-9]{2,})[[:space:]]decision=ADMIT$ ]]; then
                claim_admitted=1
                break
            fi
        done < <(grep -E '^claim: ' <<<"$block")
        (( claim_admitted )) \
            || { performance_gate_fail "performance claim is not admitted by the six-gate record: ${id}"; return 1; }
        jq -e --arg class "$id" '
            [.runs[].timings[] | select(.class == $class)
             | (.sample_count >= 30 and (.samples_ms | length) >= .sample_count
                and all(.samples_ms[]; type == "number"))]
            | length == 2 and all(. == true)
        ' "$root/$evidence" >/dev/null \
            || { performance_gate_fail "performance evidence has fewer than 30 samples per release candidate: ${id}"; return 1; }
    done <<<"$claims"
    printf '%s\n' 'performance claims gate verified'
}
