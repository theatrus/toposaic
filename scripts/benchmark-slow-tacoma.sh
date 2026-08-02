#!/usr/bin/env bash
set -euo pipefail

runs="${1:-3}"
RUST_LOG="toposaic_core::geometry=info,toposaic_api=info" \
  cargo run --release -p toposaic-api --example benchmark_setup -- \
  benchmarks/slow-tacoma.json "$runs"
