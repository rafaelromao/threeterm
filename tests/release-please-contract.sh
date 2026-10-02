#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

python3 - "${ROOT}" <<'PY'
import json
import pathlib
import re
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
assert package["version-file"] == "version.txt"
assert package["extra-files"]
assert {"type": "generic", "path": "version.txt"} in package["extra-files"]
assert config["pull-request-title-pattern"] == "chore: release ${version}"
assert set(manifest) == {"."}
release_version = manifest["."]
assert re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", release_version)
assert (root / "version.txt").read_text().strip() == release_version
assert workspace["workspace"]["package"]["version"] == release_version

extra_files = {entry["path"]: entry for entry in package["extra-files"]}
assert extra_files["Cargo.toml"]["type"] == "toml"
assert extra_files["Cargo.toml"]["jsonpath"] == "$.workspace.package.version"
assert extra_files["Cargo.lock"]["type"] == "toml"
lock_query = extra_files["Cargo.lock"]["jsonpath"]
assert lock_query.startswith("$.package[") and lock_query.endswith("].version")
assert "@.name.value ==" in lock_query

workspace_packages = {
    item["name"] for item in lock["package"]
    if item["name"] == "rehearsal" or item["name"].startswith("threeterm-")
}
assert len(workspace_packages) == 13, workspace_packages
assert all(
    item["version"] == release_version
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
raise "release workflow must have a bounded timeout" unless release.dig("jobs", "release-please", "timeout-minutes") == 300
raise "release workflow defaults must be read-only" unless
  release.dig("permissions", "contents") == "read" &&
  release.dig("permissions", "pull-requests") == "read" &&
  release.dig("permissions", "actions") == "read" &&
  release.dig("permissions", "statuses") == "read"
release_job_permissions = release.dig("jobs", "release-please", "permissions")
raise "release job must have only its required write permissions" unless
  release_job_permissions == {"actions" => "write", "contents" => "write", "pull-requests" => "write", "statuses" => "write"}
release_action = release.dig("jobs", "release-please", "steps").find do |step|
  step["uses"] == "googleapis/release-please-action@v4"
end
raise "release-please action/config is missing" unless release_action &&
  release_action.dig("with", "config-file") == "release-please-config.json" &&
  release_action.dig("with", "manifest-file") == ".release-please-manifest.json"
raise "release-please must support an approval-free token with a built-in-token fallback" unless
  release_action.dig("with", "token") == "${{ secrets.RELEASE_PLEASE_TOKEN || github.token }}"
gate_step = release.dig("jobs", "release-please", "steps").find do |step|
  step["run"].to_s.include?("release.sh verify")
end
raise "merged release PR gate verification is missing" unless gate_step &&
  gate_step.dig("env", "GH_TOKEN") == "${{ secrets.GITHUB_TOKEN }}" &&
  gate_step["run"].include?("autorelease: pending") &&
  gate_step["run"].include?("merged_at") &&
  gate_step["run"].include?("No merged Release Please PR is awaiting publication.")
gate_index = release.dig("jobs", "release-please", "steps").index(gate_step)
release_index = release.dig("jobs", "release-please", "steps").index(release_action)
raise "release gate must be verified before Release Please publishes" unless gate_index < release_index
dispatch_step = release.dig("jobs", "release-please", "steps").find do |step|
  step["run"].to_s.include?("dispatch-release-e2e.sh")
end
raise "release PR E2E dispatch step is missing" unless dispatch_step && dispatch_step.dig("env", "GH_TOKEN") == "${{ secrets.GITHUB_TOKEN }}"
checkout_index = release.dig("jobs", "release-please", "steps").index do |step|
  step["uses"] == "actions/checkout@v4"
end
dispatch_index = release.dig("jobs", "release-please", "steps").index(dispatch_step)
raise "release workflow must check out the dispatcher script before running it" unless
  checkout_index && dispatch_index && checkout_index < dispatch_index

semantic = YAML.load_file(root.join(".github/workflows/semantic-pull-request.yml"), aliases: true)
semantic_events = semantic.fetch(true)
raise "semantic title gate must run on pull requests" unless semantic_events.key?("pull_request")
raise "semantic title gate must have a bounded timeout" unless semantic.dig("jobs", "semantic-pull-request", "timeout-minutes") == 10
semantic_action = semantic.dig("jobs", "semantic-pull-request", "steps").find do |step|
  step["uses"] == "amannn/action-semantic-pull-request@v5"
end
raise "Conventional Commit title check is missing" unless semantic_action
RUBY
