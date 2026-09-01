#!/usr/bin/env bash
# Selects the serialized ThreeTerm test tier. Tests marked `slow` use libtest's
# ignore attribute, so the fast tier is the default Cargo test behavior.

set -euo pipefail

case "${1:-}" in
    fast)
        cargo test --workspace --jobs 1 -- --test-threads=1
        ;;
    slow)
        cargo test --workspace --jobs 1 -- --ignored --test-threads=1
        ;;
    *)
        echo "usage: $0 <fast|slow>" >&2
        exit 2
        ;;
esac
