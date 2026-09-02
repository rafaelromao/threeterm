#!/usr/bin/env bash
# Selects the ThreeTerm test tier. Native E2E is serialized because each test
# launches disposable native workers; fast CI has no native worker and can use
# Cargo's default parallelism.

set -euo pipefail

case "${1:-}" in
    fast)
        cargo test --workspace
        ;;
    slow)
        cargo test --workspace --jobs 1 -- --ignored --test-threads=1
        ;;
    e2e)
        cargo test --workspace --jobs 1 -- --include-ignored --test-threads=1
        ;;
    *)
        echo "usage: $0 <fast|slow|e2e>" >&2
        exit 2
        ;;
esac
