#!/usr/bin/env bash
# Profile-Guided Optimization (PGO) build for Homer.
#
# Usage: ./scripts/pgo-build.sh [target-repo-path]
#
# Requirements:
#   - Rust nightly or stable >= 1.80 (for PGO support)
#   - A target repository to profile against (default: current directory)
#
# Expected improvement: 5-15% throughput gain on computation-heavy paths.

set -euo pipefail

REPO_PATH="${1:-.}"
PGO_DIR="${CARGO_TARGET_DIR:-target}/pgo-profiles"

echo "=== Homer PGO Build ==="
echo "Profile target: ${REPO_PATH}"
echo "Profile data:   ${PGO_DIR}"
echo ""

# Step 1: Clean previous profile data
rm -rf "${PGO_DIR}"
mkdir -p "${PGO_DIR}"

# Step 2: Build instrumented binary
echo ">>> Step 1/3: Building instrumented binary..."
RUSTFLAGS="-Cprofile-generate=${PGO_DIR}" \
    cargo build --release --bin homer

# Step 3: Run instrumented binary on target repo to collect profile
echo ">>> Step 2/3: Collecting profile data..."
INSTRUMENTED_BIN="target/release/homer"

if [ -d "${REPO_PATH}/.homer" ]; then
    # Re-run update + analysis on existing homer DB
    "${INSTRUMENTED_BIN}" update --path "${REPO_PATH}" || true
    "${INSTRUMENTED_BIN}" status --path "${REPO_PATH}" || true
    "${INSTRUMENTED_BIN}" query --path "${REPO_PATH}" "show hotspots" || true
else
    # Initialize and run full extraction + analysis
    "${INSTRUMENTED_BIN}" init --path "${REPO_PATH}" || true
    "${INSTRUMENTED_BIN}" update --path "${REPO_PATH}" || true
    "${INSTRUMENTED_BIN}" status --path "${REPO_PATH}" || true
fi

# Step 4: Merge profile data
echo ">>> Merging profile data..."
if command -v llvm-profdata &> /dev/null; then
    llvm-profdata merge -o "${PGO_DIR}/merged.profdata" "${PGO_DIR}"
elif command -v rust-profdata &> /dev/null; then
    rust-profdata merge -o "${PGO_DIR}/merged.profdata" "${PGO_DIR}"
else
    echo "Warning: neither llvm-profdata nor rust-profdata found."
    echo "Using raw profile data (may still work with LLVM >= 18)."
fi

# Step 5: Rebuild with profile data
echo ">>> Step 3/3: Building optimized binary with PGO..."
if [ -f "${PGO_DIR}/merged.profdata" ]; then
    RUSTFLAGS="-Cprofile-use=${PGO_DIR}/merged.profdata" \
        cargo build --release --bin homer
else
    RUSTFLAGS="-Cprofile-use=${PGO_DIR}" \
        cargo build --release --bin homer
fi

echo ""
echo "=== PGO build complete ==="
echo "Binary: target/release/homer"
echo ""
echo "Verify with: cargo bench -p homer-core"
