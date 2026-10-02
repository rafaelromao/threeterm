#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

ruby -ryaml -rpathname -ropen3 -rtmpdir -rjson - "${ROOT}" <<'RUBY'
root = Pathname.new(ARGV.fetch(0))
workflow = YAML.load_file(root.join(".github/workflows/e2e.yml"), aliases: true)
events = workflow.fetch(true)
raise "native E2E must use dispatch only to avoid duplicate App/PAT push runs" unless
  events.keys == ["workflow_dispatch"]
journeys = YAML.load_file(root.join(".github/workflows/three-journey.yml"), aliases: true)
raise "three-journey gate must use dispatch only to avoid duplicate App/PAT push runs" unless
  journeys.fetch(true).keys == ["workflow_dispatch"]
%w[tui aggregate].each do |job_name|
  raise "#{job_name} must still publish failure evidence after a producer fails" unless
    journeys.dig("jobs", job_name, "if") == "${{ always() }}"
end

job = workflow.fetch("jobs").fetch("native-e2e")
expected_result = job.dig("env", "THREETERM_EXPECTED_ACCEPTANCE_RESULT")
raise "native E2E must honor the dispatcher's expected catalog result" unless
  expected_result == "${{ inputs.expected_catalog_result || 'failed' }}" &&
  events.dig("workflow_dispatch", "inputs", "expected_catalog_result", "options") == ["failed", "passed"]
raise "native job must defer graphical coverage to the three-journey gate" unless
  job.dig("env", "THREETERM_ACCEPTANCE_SCOPE") == "native"

commands = job.fetch("steps").filter_map { |step| step["run"] }.join("\n")
raise "native acceptance catalog must remain in the release E2E job" unless commands.include?("acceptance.sh")
raise "full ignored E2E suite must run in the release E2E job" unless commands.include?("e2e.sh")
raise "expected acceptance failure must not suppress the full E2E suite" unless
  commands.include?("acceptance_status=$?") &&
  commands.index("acceptance.sh") < commands.index("e2e.sh")
raise "acceptance and full E2E outcomes must be checked independently" unless
  commands.include?("release-acceptance-status") &&
  commands.include?("release-e2e-status") &&
  commands.include?("check-native-e2e-status.sh")

# Exercise the actual YAML shell through both quoting layers, replacing only
# native build/container prerequisites. Syntax checks cannot catch a printf
# format whose single quotes were consumed by the enclosing bash -lc string.
container_step = job.fetch("steps").find { |step| step["run"].to_s.include?("podman run") }.fetch("run")
stubs = <<~'SH'
  pacman() { :; }
  rustup() { printf '%s\n' "$(<rust-toolchain-channel.txt)"; }
  bash() {
    case "$1" in
      */acceptance.sh) return "${TEST_ACCEPTANCE_EXIT}" ;;
      */e2e.sh) return "${TEST_E2E_EXIT}" ;;
      *) command bash "$@" ;;
    esac
  }
  podman() { ( eval "${@: -1}" ); }
SH
[["passed", 0, 0, 0], ["failed", 1, 0, 0],
 ["passed", 1, 0, 1], ["failed", 1, 7, 7]].each do |expected, acceptance_exit, e2e_exit, expected_exit|
  Dir.mktmpdir("threeterm-native-workflow-") do |directory|
    fixture = Pathname.new(directory)
    fixture.join("target").mkpath
    fixture.join("rust-toolchain-channel.txt").write("1.97.1\n")
    fixture.join(".github/scripts").mkpath
    fixture.join(".github/scripts/check-native-e2e-status.sh").write(
      root.join(".github/scripts/check-native-e2e-status.sh").read)
    env = {"HOME" => directory, "THREETERM_EXPECTED_ACCEPTANCE_RESULT" => expected,
           "THREETERM_ACCEPTANCE_GATE_TIMEOUT_SECONDS" => "1",
           "THREETERM_ACCEPTANCE_SCOPE" => "native",
           "THREETERM_COVERAGE_EVIDENCE_ROOT" => "target/coverage",
           "TEST_ACCEPTANCE_EXIT" => acceptance_exit.to_s, "TEST_E2E_EXIT" => e2e_exit.to_s}
    stdout, stderr, status = Open3.capture3(env, "bash", "-c", "#{stubs}\n#{container_step}", chdir: directory)
    raise "container status handoff failed: #{stdout}#{stderr}" unless status.exitstatus == expected_exit
    {"release-acceptance-status" => acceptance_exit, "release-e2e-status" => e2e_exit}.each do |file, code|
      actual = fixture.join("target", file).read
      raise "#{file} must contain a newline-terminated integer, got #{actual.inspect}" unless actual == "#{code}\n"
    end
  end
end

# Validate the actual release-catalog predicate: native evidence may defer only
# the graphical gate, while namespace and every other gate stay fail-closed.
validation_step = job.fetch("steps").find { |step| step["name"] == "Validate acceptance catalog" }.fetch("run")
gate_ids = %w[source-integrity worker.native baseline workflow.l-bracket workflow.box-with-lid
              workflow.reusable-component workflow.historical-edit workflow.object-timeline
              workflow.keyboard-first workflow.invalid-edit-recovery replay.canonical-extrude
              replay.boolean-pattern replay.reattach-edge reliability.failure-drill
              registry.command-schemas schema.identities licensing.libslvs release.namespace
              documentation.workspace performance.claims]
base_catalog = {"schema_version" => "threeterm.acceptance.catalog/1", "scope" => "native",
                "deferred_gates" => ["coverage.all-surfaces"], "result" => "passed",
                "source" => {"commit" => "a" * 40, "changed_during_run" => false},
                "evidence" => {"complete" => true},
                "gates" => gate_ids.map { |id| {"id" => id, "status" => "passed", "timed_out" => false} },
                "artifacts" => [{"bytes" => 1, "sha256" => "b" * 64}]}
cases = [
  ["native catalog passes", "passed", true, ->(catalog) {}],
  ["full scope cannot masquerade as native", "passed", false, ->(catalog) { catalog["scope"] = "full" }],
  ["only graphical coverage may be deferred", "passed", false, ->(catalog) { catalog["deferred_gates"] << "release.namespace" }],
  ["incomplete native evidence fails", "passed", false, ->(catalog) { catalog["evidence"]["complete"] = false }],
  ["missing gate fails", "passed", false, ->(catalog) { catalog["gates"].shift }],
  ["timed-out gate fails", "passed", false, ->(catalog) { catalog["gates"][0]["timed_out"] = true }],
  ["unsigned release fails release mode", "passed", false, ->(catalog) {
    catalog["result"] = "failed"
    catalog["gates"].find { |gate| gate["id"] == "release.namespace" }["status"] = "failed"
  }],
  ["unsigned release matches manual expected-failure mode", "failed", true, ->(catalog) {
    catalog["result"] = "failed"
    catalog["gates"].find { |gate| gate["id"] == "release.namespace" }["status"] = "failed"
  }],
  ["other failures cannot hide behind an unsigned gate", "failed", false, ->(catalog) {
    catalog["result"] = "failed"
    catalog["gates"].find { |gate| gate["id"] == "release.namespace" }["status"] = "failed"
    catalog["gates"][0]["status"] = "failed"
  }]
]
cases.each do |name, expected, should_pass, mutate|
  Dir.mktmpdir("threeterm-native-catalog-") do |directory|
    fixture = Pathname.new(directory)
    fixture.join("target/acceptance-run/logs").mkpath
    fixture.join("target/acceptance-run/logs/release.namespace.log").write(
      "release gate: current gate is missing required field: ^Current gate status: `SIGNED`$\n")
    catalog = JSON.parse(JSON.generate(base_catalog))
    mutate.call(catalog)
    fixture.join("target/acceptance-catalog.json").write(JSON.generate(catalog))
    env = {"THREETERM_EXPECTED_ACCEPTANCE_RESULT" => expected, "TEST_COMMIT" => "a" * 40}
    stdout, stderr, status = Open3.capture3(env, "bash", "-c",
      "git() { printf '%s\\n' \"${TEST_COMMIT}\"; }\n#{validation_step}", chdir: directory)
    raise "#{name}: #{stdout}#{stderr}" unless status.success? == should_pass
  end
end
RUBY

grep -Fq 'bash .github/scripts/test-suite.sh e2e' "${ROOT}/.github/scripts/e2e.sh"
