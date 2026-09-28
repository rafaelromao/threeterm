#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

ruby -ryaml -rpathname - "${ROOT}" <<'RUBY'
root = Pathname.new(ARGV.fetch(0))
workflow = YAML.load_file(root.join(".github/workflows/e2e.yml"), aliases: true)
events = workflow.fetch(true)
raise "manual native E2E dispatch must remain enabled" unless events.key?("workflow_dispatch")

pull_request = events.fetch("pull_request")
types = pull_request.fetch("types")
%w[opened reopened synchronize labeled].each do |type|
  raise "native E2E workflow must observe #{type} pull request events" unless types.include?(type)
end

job = workflow.fetch("jobs").fetch("native-e2e")
condition = job.fetch("if")
%w[workflow_dispatch pull_request autorelease: pending].each do |required|
  raise "native E2E job is missing its release-only #{required} condition" unless condition.include?(required)
end

commands = job.fetch("steps").filter_map { |step| step["run"] }.join("\n")
raise "native acceptance catalog must remain in the release E2E job" unless commands.include?("acceptance.sh")
raise "full ignored E2E suite must run in the release E2E job" unless commands.include?("e2e.sh")
raise "expected acceptance failure must not suppress the full E2E suite" unless
  commands.include?("acceptance_status=$?") &&
  commands.index("acceptance.sh") < commands.index("e2e.sh")

eligible = lambda do |event, labels|
  event == "workflow_dispatch" ||
    (event == "pull_request" && labels.include?("autorelease: pending"))
end
raise "ordinary feature PR must not run native E2E" if eligible.call("pull_request", [])
raise "release PR must run native E2E" unless eligible.call("pull_request", ["autorelease: pending"])
raise "manual dispatch must run native E2E" unless eligible.call("workflow_dispatch", [])
RUBY

grep -Fq 'bash .github/scripts/test-suite.sh e2e' "${ROOT}/.github/scripts/e2e.sh"
