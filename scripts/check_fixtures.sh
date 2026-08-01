#!/usr/bin/env bash
# Dual-end fixture alignment toolchain (M0, docs/IMPLEMENTATION_PLAN.md §2).
#
# Any change under docs/storage_fixtures/ must keep BOTH implementations green:
#   1. Rust fixture tests in this repo (cargo test)
#   2. Dart fixture tests in the ShePaw app repo (flutter test)
#
# Usage:
#   scripts/check_fixtures.sh            # Rust side only
#   SHEPAW_REPO=/path/to/shepaw scripts/check_fixtures.sh   # dual-end gate
set -euo pipefail
cd "$(dirname "$0")/.."

echo "[1/2] Rust fixture tests..."
cargo test fixture

echo "[2/2] Dart fixture tests (ShePaw repo)..."
if [ -n "${SHEPAW_REPO:-}" ]; then
  (cd "$SHEPAW_REPO" && flutter test test/storage/store_protocol_test.dart)
else
  echo "SHEPAW_REPO not set — skipping Dart side (set it to run the dual-end gate)."
fi

echo "fixture check complete"
