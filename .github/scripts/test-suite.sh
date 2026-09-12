#!/usr/bin/env bash
# Selects the ThreeTerm test tier. Native E2E is serialized because each test
# launches disposable native workers; fast CI has no native worker and can use
# Cargo's default parallelism.

set -euo pipefail

case "${1:-}" in
    fast)
        # These acceptance tests intentionally fail closed when the immutable
        # OCCT worker is absent; native-e2e runs them unskipped with
        # THREETERM_REQUIRE_OCCT=1 and the worker installed.
        cargo test --workspace -- \
            --skip supervised_occt_extrude \
            --skip required_occt_worker \
            --skip generation_identity \
            --skip generation_publication \
            --skip generation_interruption_recovery \
            --skip interactive_shared_command_semantics \
            --skip interactive_production_event_loop
        ;;
    slow)
        cargo test --workspace --jobs 1 -- --ignored --test-threads=1
        ;;
    e2e)
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test --workspace --jobs 1 -- --include-ignored --test-threads=1
        ;;
    *)
        echo "usage: $0 <fast|slow|e2e>" >&2
        exit 2
        ;;
esac
