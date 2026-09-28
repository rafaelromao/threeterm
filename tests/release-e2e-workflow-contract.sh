#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

ruby -ryaml -rpathname - "${ROOT}" <<'RUBY'
root = Pathname.new(ARGV.fetch(0))
workflow = YAML.load_file(root.join(".github/workflows/e2e.yml"), aliases: true)
events = workflow.fetch(true)
raise "manual native E2E dispatch must remain enabled" unless events.key?("workflow_dispatch")
raise "native E2E must not start on every pull request" if events.key?("pull_request")
release_push_branches = events.dig("push", "branches")
raise "native E2E push trigger must match Release Please branches" unless
  release_push_branches == ["release-please--branches--**"]

job = workflow.fetch("jobs").fetch("native-e2e")
condition = job.fetch("if")
%w[workflow_dispatch push release-please--branches-- github.ref_name].each do |required|
  raise "native E2E job is missing its release-only #{required} condition" unless condition.include?(required)
end
expected_result = job.dig("env", "THREETERM_EXPECTED_ACCEPTANCE_RESULT")
raise "release PR native E2E must require a passing acceptance catalog" unless
  expected_result.include?("push") && expected_result.include?("passed") && expected_result.include?("failed")

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

eligible = lambda do |event, ref_name = ""|
  event == "workflow_dispatch" ||
    (event == "push" && ref_name.start_with?("release-please--branches--"))
end
raise "pull request must not run native E2E" if eligible.call("pull_request", "feature/feature-pr")
raise "ordinary main push must not run native E2E" if eligible.call("push", "main")
raise "release branch push must run native E2E" unless
  eligible.call("push", "release-please--branches--main--components--threeterm")
raise "manual dispatch must run native E2E" unless eligible.call("workflow_dispatch", [])
RUBY

grep -Fq 'bash .github/scripts/test-suite.sh e2e' "${ROOT}/.github/scripts/e2e.sh"
