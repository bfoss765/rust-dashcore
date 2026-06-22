#!/bin/bash
# Extend ./chain-data to BENCH_HEIGHT (downloads only the missing blocks) using the dashd that
# ships INSIDE the peer docker image — so no host dashd / DASHD_PATH / setup-dashd.py is needed,
# only docker. BENCH_HEIGHT comes from the environment (run.sh exports it from the scenario's
# `blocks`). There is no .env.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHAIN_DIR="${SCRIPT_DIR}/chain-data"
SUBDIR="testnet3"
IMAGE="dash-spv-bench/dashd:latest"

cd "${SCRIPT_DIR}"

: "${BENCH_HEIGHT:?BENCH_HEIGHT must be set — run.sh exports it from the scenario blocks}"
command -v docker >/dev/null 2>&1 || { echo "Error: docker is required to build the snapshot." >&2; exit 1; }

# Same image the peers use; build it if we don't have it yet.
docker image inspect "${IMAGE}" >/dev/null 2>&1 \
  || { echo "==> building peer image"; docker build -t "${IMAGE}" -f "${SCRIPT_DIR}/Dockerfile" "${SCRIPT_DIR}"; }

mkdir -p "${CHAIN_DIR}"

echo "==> extending snapshot to height ${BENCH_HEIGHT} via docker (downloads only the missing blocks)..."

# One-shot container: dashd syncs testnet to -stopatheight then exits. Idempotent — if ./chain-data
# is already at (or past) the height it stops right away. --user makes dashd write ./chain-data as
# the host user (not root), so the later CoW clone can read it.
docker run --rm --user "$(id -u):$(id -g)" -v "${CHAIN_DIR}:/data" "${IMAGE}" \
  dashd -testnet -datadir=/data -daemon=0 -server=1 \
  -blockfilterindex=1 -peerblockfilters=1 -stopatheight="${BENCH_HEIGHT}" \
  -txindex=0 -prune=0 -disablewallet=1

echo "==> done. Snapshot is at ${CHAIN_DIR}/${SUBDIR} (height ${BENCH_HEIGHT})."
