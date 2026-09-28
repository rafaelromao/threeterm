#!/usr/bin/env bash

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SPEC="$ROOT/docs/mvp-implementation-specification.md"

fail() {
    printf 'mvp specification contract: %s\n' "$1" >&2
    exit 1
}

[[ -f "$SPEC" ]] || fail "missing $SPEC"

require() {
    local anchor="$1"
    grep --fixed-strings --quiet -- "$anchor" "$SPEC" \
        || fail "missing required contract anchor: $anchor"
}

require '# ThreeTerm MVP Implementation Specification'
require '## Authority and Scope'
require '## Domain and Persistence'
require '## Worker and Command Boundaries'
require '## Validation Journeys'
require '## Interactive Presentation and Caching'
require '## Themes, Lua, and Release Gates'
require '## Module and Work Map'
require '## Verification Contract'
require 'https://github.com/rafaelromao/threeterm/issues/58'
require '`Revision Snapshot`'
require '`Project Generation`'
require '`Derived Result`'
require '`Stale Last-Valid Geometry`'
require 'manifest.json'
require 'transactions.log'
require 'project.previous-generation/'
require 'two-epoch forward-only migration'
require 'unknown fields, feature kinds, command versions, worker identities'
require 'OCCT worker'
require 'libslvs'
require 'cooperative cancellation'
require 'threeterm.command.bracket/1'
require 'CLI and MCP'
require 'direct local Ghostty'
require 'Kitty RGB/zlib'
require 'one in-flight frame'
require 'Catppuccin dark'
require 'TokyoNight dark'
require 'EverGreen dark'
require 'GruvBox dark'
require 'Sandman light'
require 'six gates'
require 'trademark and namespace gate'

for journey in \
    '### 1. L-shaped bracket' \
    '### 2. Box with lid' \
    '### 3. Reusable component' \
    '### 4. Historical edit' \
    '### 5. Object-specific timeline' \
    '### 6. Keyboard-first modeling' \
    '### 7. Recovery from invalid edit'
do
    require "$journey"
done

printf 'mvp specification contract: pass\n'
