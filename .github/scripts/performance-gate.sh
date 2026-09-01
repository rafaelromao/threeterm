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
    grep -Eiq 'performance|latency|throughput|startup|memory|benchmark|(^|[^[:alnum:]])p(50|95|99)([^[:alnum:]]|$)|(^|[^[:alnum:]])[0-9]+([.,][0-9]+)?[[:space:]]*(ms|millisecond|milliseconds|us|microsecond|microseconds|秒)([^[:alnum:]]|$)'
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
    grep -Fxq "release_commit: ${expected_commit}" <<<"$block" \
        || { performance_gate_fail 'six-gate record is bound to a different commit'; return 1; }
    grep -Fxq "release_tag: ${expected_tag}" <<<"$block" \
        || { performance_gate_fail 'six-gate record is bound to a different tag'; return 1; }
    local today
    today="$(date -u +%F)"
    grep -Eq "^record_date: ${today}$" <<<"$block" \
        || { performance_gate_fail 'six-gate record is not current'; return 1; }

    local evidence evidence_digest actual owner signature gate
    evidence="$(grep -E '^evidence_path: ' <<<"$block" | cut -d' ' -f2-)"
    evidence_digest="$(grep -E '^evidence_sha256: ' <<<"$block" | cut -d' ' -f2-)"
    [[ -n "$evidence" && "$evidence" != /* && "$evidence" != *..* && -f "$root/$evidence" ]] \
        || { performance_gate_fail 'six-gate evidence path is missing or unsafe'; return 1; }
    [[ "$evidence_digest" =~ ^[0-9a-f]{64}$ ]] \
        || { performance_gate_fail 'six-gate evidence digest is invalid'; return 1; }
    actual="$(sha256sum "$root/$evidence" | cut -d' ' -f1)"
    [[ "$actual" == "$evidence_digest" ]] \
        || { performance_gate_fail 'six-gate evidence digest does not match'; return 1; }
    owner="$(grep -E '^owner: ' <<<"$block" | cut -d' ' -f2-)"
    signature="$(grep -E '^record_signature: ' <<<"$block" | cut -d' ' -f2-)"
    [[ -n "$owner" && "$owner" == "$signature" ]] \
        || { performance_gate_fail 'six-gate record signature does not identify the owner'; return 1; }
    for gate in 1 2 3 4 5 6; do
        grep -Fxq "gate_${gate}: PASS" <<<"$block" \
            || { performance_gate_fail "six-gate ${gate} did not pass"; return 1; }
        grep -Fxq "gate_${gate}_signature: ${owner}" <<<"$block" \
            || { performance_gate_fail "six-gate ${gate} is not signed by the owner"; return 1; }
    done

    local claims claim id metric unit percentile fixture scale row field
    claims="$(performance_gate_claim_lines <"$material")"
    [[ -n "$claims" ]] || { performance_gate_fail 'performance claim must use the structured claim grammar'; return 1; }
    while IFS= read -r claim; do
        [[ "$claim" =~ ^ThreeTerm\ performance\ claim:\ .+$ ]] || continue
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
        row="claim: id=${id} metric=${metric} unit=${unit} percentile=${percentile} fixture=${fixture} scale=${scale} decision=ADMIT"
        grep -Fxq "$row" <<<"$block" \
            || { performance_gate_fail "performance claim is not admitted by the six-gate record: ${id}"; return 1; }
    done <<<"$claims"
    printf '%s\n' 'performance claims gate verified'
}
