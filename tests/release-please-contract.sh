#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

python3 - "${ROOT}" <<'PY'
import json
import pathlib
import sys
import tomllib

root = pathlib.Path(sys.argv[1])
config = json.loads((root / "release-please-config.json").read_text())
manifest = json.loads((root / ".release-please-manifest.json").read_text())
workspace = tomllib.loads((root / "Cargo.toml").read_text())
lock = tomllib.loads((root / "Cargo.lock").read_text())

package = config["packages"]["."]
assert package["release-type"] == "simple"
assert package["package-name"] == "threeterm"
assert package["changelog-path"] == "CHANGELOG.md"
assert manifest == {".": "0.1.0"}
assert (root / "version.txt").read_text().strip() == manifest["."]
assert workspace["workspace"]["package"]["version"] == manifest["."]

extra_files = {entry["path"]: entry for entry in package["extra-files"]}
assert extra_files["Cargo.toml"]["type"] == "toml"
assert extra_files["Cargo.toml"]["jsonpath"] == "$.workspace.package.version"
assert extra_files["Cargo.lock"]["type"] == "toml"
lock_query = extra_files["Cargo.lock"]["jsonpath"]
assert lock_query.startswith("$.package[") and lock_query.endswith("].version")

workspace_packages = {
    item["name"] for item in lock["package"]
    if item["name"] == "rehearsal" or item["name"].startswith("threeterm-")
}
assert len(workspace_packages) == 13, workspace_packages
assert all(
    item["version"] == manifest["."]
    for item in lock["package"]
    if item["name"] in workspace_packages
)
assert all(name in lock_query for name in workspace_packages), lock_query
PY

ruby -ryaml -rpathname - "${ROOT}" <<'RUBY'
root = Pathname.new(ARGV.fetch(0))
release = YAML.load_file(root.join(".github/workflows/release-please.yml"), aliases: true)
release_events = release.fetch(true)
raise "release workflow must run on main pushes" unless release_events.dig("push", "branches") == ["main"]
raise "release workflow must write repository contents" unless release.dig("permissions", "contents") == "write"
raise "release workflow must open pull requests" unless release.dig("permissions", "pull-requests") == "write"
release_action = release.dig("jobs", "release-please", "steps").find do |step|
  step["uses"] == "googleapis/release-please-action@v4"
end
raise "release-please action/config is missing" unless release_action &&
  release_action.dig("with", "config-file") == "release-please-config.json" &&
  release_action.dig("with", "manifest-file") == ".release-please-manifest.json"

semantic = YAML.load_file(root.join(".github/workflows/semantic-pull-request.yml"), aliases: true)
semantic_events = semantic.fetch(true)
raise "semantic title gate must run on pull requests" unless semantic_events.key?("pull_request")
semantic_action = semantic.dig("jobs", "semantic-pull-request", "steps").find do |step|
  step["uses"] == "amannn/action-semantic-pull-request@v5"
end
raise "Conventional Commit title check is missing" unless semantic_action
RUBY
