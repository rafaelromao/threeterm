#!/usr/bin/env bash
# Deterministic release bundle construction and verification.

set -euo pipefail

release_artifact_fail() {
    printf 'release artifact: %s\n' "$1" >&2
    return 1
}

release_artifact_sha256() {
    sha256sum "$1" | cut -d' ' -f1
}

release_artifact_require_safe_tree() {
    local root="$1"
    local link
    link="$(find "$root" -type l -print -quit)"
    [[ -z "$link" ]] || { release_artifact_fail "symlink is not allowed in release bundle: ${link}"; return 1; }
}

release_artifact_write_archive() {
    local root="$1"
    local archive="$2"
    LC_ALL=C TZ=UTC tar --sort=name --format=ustar --mtime='1970-01-01 00:00:00Z' \
        --owner=0 --group=0 --numeric-owner \
        -cf - -C "$root" release-manifest.json worker-manifest.json libslvs-artifact \
        | LC_ALL=C TZ=UTC gzip -n -c >"$archive"
}

release_artifact_file_list() {
    local root="$1"
    find "$root/libslvs-artifact" -type f -printf '%P\n' \
        | LC_ALL=C sort \
        | while IFS= read -r path; do
            printf 'libslvs-artifact/%s\n' "$path"
        done
}

build_release_bundle() {
    local source_root="$1"
    local tag="$2"
    local expected_commit="$3"
    local artifact_root="$4"
    local output_root="$5"
    local input_manifest="${6:-${artifact_root}/manifest.json}"
    local actual_commit worker_manifest worker_manifest_digest

    [[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]] \
        || { release_artifact_fail "invalid release tag: ${tag}"; return 1; }
    actual_commit="$(git -C "$source_root" rev-parse HEAD)" \
        || { release_artifact_fail 'unable to resolve source commit'; return 1; }
    [[ "$actual_commit" == "$expected_commit" && "$expected_commit" =~ ^[0-9a-f]{40}$ ]] \
        || { release_artifact_fail 'release bundle source commit is not the verified checkout commit'; return 1; }

    worker_manifest="$input_manifest"
    [[ -f "$worker_manifest" ]] || { release_artifact_fail 'verified worker manifest is missing'; return 1; }
    verify_libslvs_artifact "$worker_manifest" "$artifact_root" >/dev/null \
        || { release_artifact_fail 'verified worker artifact is invalid'; return 1; }
    [[ "$(jq -er '.artifact.source_revision' "$worker_manifest")" == "$expected_commit" ]] \
        || { release_artifact_fail 'worker artifact is bound to a different source commit'; return 1; }

    rm -rf "$output_root"
    mkdir -p "$output_root"
    cp -a "$artifact_root" "$output_root/libslvs-artifact"
    release_artifact_require_safe_tree "$output_root"
    find "$output_root" -type d -exec chmod 755 {} +
    find "$output_root" -type f -exec chmod 644 {} +
    chmod 755 "$output_root/libslvs-artifact/bin/threeterm-slvs-worker"
    cp "$output_root/libslvs-artifact/manifest.json" "$output_root/worker-manifest.json"
    worker_manifest_digest="$(release_artifact_sha256 "$output_root/worker-manifest.json")"

    local files='[]' path digest
    while IFS= read -r path; do
        digest="$(release_artifact_sha256 "$output_root/$path")"
        files="$(jq -c --arg path "$path" --arg sha256 "$digest" \
            '. + [{path: $path, sha256: $sha256}]' <<<"$files")"
    done < <(release_artifact_file_list "$output_root")
    files="$(jq -c --arg path worker-manifest.json --arg sha256 "$worker_manifest_digest" \
        '. + [{path: $path, sha256: $sha256}]' <<<"$files")"

    jq -S -n \
        --arg schema_version threeterm.release/1 \
        --arg repository https://github.com/rafaelromao/threeterm \
        --arg commit "$expected_commit" \
        --arg tag "$tag" \
        --arg worker_manifest_path worker-manifest.json \
        --arg worker_manifest_sha256 "$worker_manifest_digest" \
        --arg worker_id "$(jq -er '.artifact.worker_id' "$worker_manifest")" \
        --arg worker_schema "$(jq -er '.artifact.worker_schema_version' "$worker_manifest")" \
        --arg license_basis "$(jq -er '.licensing.basis' "$worker_manifest")" \
        --arg archive "threeterm-${tag}.tar.gz" \
        --argjson files "$files" \
        '{schema_version: $schema_version, repository: $repository, commit: $commit,
          tag: $tag, worker: {id: $worker_id, schema: $worker_schema,
          licensing_basis: $license_basis, licensing_manifest: $worker_manifest_path,
          licensing_manifest_sha256: $worker_manifest_sha256}, archive: $archive,
          files: $files}' >"$output_root/release-manifest.json"
    chmod 644 "$output_root/release-manifest.json"

    release_artifact_write_archive "$output_root" "$output_root/threeterm-${tag}.tar.gz"
    {
        for path in release-manifest.json worker-manifest.json; do
            printf '%s  %s\n' "$(release_artifact_sha256 "$output_root/$path")" "$path"
        done
        while IFS= read -r path; do
            printf '%s  %s\n' "$(release_artifact_sha256 "$output_root/$path")" "$path"
        done < <(release_artifact_file_list "$output_root")
        path="threeterm-${tag}.tar.gz"
        printf '%s  %s\n' "$(release_artifact_sha256 "$output_root/$path")" "$path"
    } | LC_ALL=C sort >"$output_root/SHA256SUMS"

    verify_release_bundle "$output_root" "$tag" "$expected_commit"
}

verify_release_bundle() {
    local root="$1"
    local expected_tag="$2"
    local expected_commit="$3"
    local manifest="$root/release-manifest.json"
    local path expected actual
    [[ -f "$manifest" && -f "$root/SHA256SUMS" && -f "$root/worker-manifest.json" ]] \
        || { release_artifact_fail 'release bundle is missing a required catalog file'; return 1; }
    release_artifact_require_safe_tree "$root"
    [[ "$(jq -er '.schema_version' "$manifest")" == threeterm.release/1 ]] \
        || { release_artifact_fail 'unsupported release manifest schema'; return 1; }
    [[ "$(jq -er '.repository' "$manifest")" == https://github.com/rafaelromao/threeterm ]] \
        || { release_artifact_fail 'release manifest repository identity is invalid'; return 1; }
    [[ "$(jq -er '.commit' "$manifest")" == "$expected_commit" \
        && "$(jq -er '.tag' "$manifest")" == "$expected_tag" ]] \
        || { release_artifact_fail 'release manifest is not bound to the verified commit and tag'; return 1; }
    [[ "$(jq -er '.worker.licensing_manifest' "$manifest")" == worker-manifest.json ]] \
        || { release_artifact_fail 'release manifest licensing manifest path is invalid'; return 1; }
    [[ "$(jq -er '.worker.licensing_manifest_sha256' "$manifest")" == \
        "$(release_artifact_sha256 "$root/worker-manifest.json")" ]] \
        || { release_artifact_fail 'release manifest licensing manifest digest is invalid'; return 1; }
    verify_libslvs_artifact "$root/libslvs-artifact/manifest.json" "$root/libslvs-artifact" >/dev/null \
        || { release_artifact_fail 'release bundle worker artifact is invalid'; return 1; }
    [[ "$(jq -er '.artifact.source_revision' "$root/worker-manifest.json")" == "$expected_commit" ]] \
        || { release_artifact_fail 'release worker artifact is bound to a different source commit'; return 1; }
    [[ "$(jq -er '.worker.id' "$manifest")" == slvs \
        && "$(jq -er '.worker.schema' "$manifest")" == threeterm.workers.slvs/1 \
        && "$(jq -er '.worker.licensing_basis' "$manifest")" == GPL-3.0-only ]] \
        || { release_artifact_fail 'release manifest worker or licensing identity is invalid'; return 1; }
    [[ "$(jq -er '.archive' "$manifest")" == "threeterm-${expected_tag}.tar.gz" ]] \
        || { release_artifact_fail 'release archive name is invalid'; return 1; }

    local actual_files expected_files catalog_files
    actual_files="$(find "$root" -type f -printf '%P\n' | LC_ALL=C sort)"
    expected_files="$(
        {
            printf '%s\n' release-manifest.json SHA256SUMS
            printf '%s\n' "$(jq -er '.archive' "$manifest")"
            jq -r '.files[].path' "$manifest"
        } | LC_ALL=C sort
    )"
    [[ "$actual_files" == "$expected_files" ]] \
        || { release_artifact_fail 'release bundle contains an unlisted file'; return 1; }
    catalog_files="$(sed -E 's/^[0-9a-f]{64}  //' "$root/SHA256SUMS" | LC_ALL=C sort)"
    [[ "$catalog_files" == "$(printf '%s\n' "$actual_files" | grep -Fxv SHA256SUMS)" ]] \
        || { release_artifact_fail 'checksum catalog does not cover the complete release bundle'; return 1; }

    while IFS=$'\t' read -r path expected; do
        [[ "$path" != /* && "$path" != *..* && -n "$path" ]] \
            || { release_artifact_fail "unsafe release manifest path: ${path}"; return 1; }
        [[ -f "$root/$path" && ! -L "$root/$path" ]] \
            || { release_artifact_fail "release manifest file is missing: ${path}"; return 1; }
        actual="$(release_artifact_sha256 "$root/$path")"
        [[ "$actual" == "$expected" ]] \
            || { release_artifact_fail "release manifest digest mismatch: ${path}"; return 1; }
    done < <(jq -r '.files[] | [.path, .sha256] | @tsv' "$manifest")

    local sum line_digest line_path
    while read -r line_digest line_path; do
        [[ "$line_path" != SHA256SUMS && -f "$root/$line_path" && ! -L "$root/$line_path" ]] \
            || { release_artifact_fail "invalid checksum catalog entry: ${line_path}"; return 1; }
        sum="$(release_artifact_sha256 "$root/$line_path")"
        [[ "$sum" == "$line_digest" ]] \
            || { release_artifact_fail "checksum catalog mismatch: ${line_path}"; return 1; }
    done <"$root/SHA256SUMS"

    local recreated
    recreated="$(mktemp)"
    release_artifact_write_archive "$root" "$recreated"
    cmp "$recreated" "$root/$(jq -er '.archive' "$manifest")" \
        || { rm -f "$recreated"; release_artifact_fail 'release archive is not reproducible'; return 1; }
    rm -f "$recreated"
    printf '%s\n' 'release artifact bundle verified'
}
