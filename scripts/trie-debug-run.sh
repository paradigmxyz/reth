#!/usr/bin/env bash
#
# trie-debug-run.sh
#
# Downloads the oldest available eth mainnet minimal snapshot (if needed),
# compiles reth with trie-debug support, runs reth node with proof-jitter,
# benchmarks N blocks, and watches for trie-debug artifacts.
#
# By default uses a regular datadir. Optionally uses schelk for instant
# volume restore when --virgin and --scratch are provided.
#
# Intended to run on dev-brian from the root of a reth checkout.
#
# References:
#   https://tempoxyz.slack.com/archives/C0A0ZCUHMNY/p1773225066924499
#   https://tempoxyz.enterprise.slack.com/archives/C09FQDW2ZRP/p1773141477638759

set -euo pipefail

# -- CLI argument parsing -----------------------------------------------------
ADVANCE_BLOCKS=10000
VIRGIN_DEV=""
SCRATCH_DEV=""
DATADIR="$HOME/.local/share/reth/mainnet-trie-debug"
MOUNT_POINT="/schelk"
RAMDISK="/dev/ram0"
SKIP_SYNC=false
EXTRA_RETH_ARGS=()

usage() {
    cat <<'EOF'
trie-debug-run.sh -- Run reth with trie-debug and proof-jitter, benchmarking
against the oldest available mainnet snapshot.

USAGE:
  ./scripts/trie-debug-run.sh [OPTIONS] [-- EXTRA_RETH_ARGS...]

OPTIONS:
  -n, --blocks NUM           Number of blocks to advance (default: 10000)
  --datadir DIR              Reth data directory (default: ~/.local/share/reth/mainnet-trie-debug)
  --skip-sync                Skip snapshot download, use existing datadir as-is
  -h, --help                 Show this help message

  Everything after "--" is passed as extra arguments to `reth node`.

SCHELK OPTIONS (optional, enables instant volume restore):
  --virgin DEV               Block device for the virgin (golden image) volume
  --scratch DEV              Block device for the scratch (working) volume
  --mount-point DIR          Mount point for scratch volume (default: /schelk)
  --ramdisk DEV              Ramdisk device for dm-era metadata (default: /dev/ram0)

BEHAVIOR:
  WITH --skip-sync:
    Skips all snapshot download/restore. Uses the existing --datadir as-is
    (it must already contain a db/ directory from a prior run).

  WITHOUT --virgin/--scratch (default):
    Downloads the oldest available weekly snapshot from minio into --datadir,
    overwriting any existing data. Then compiles reth, runs the benchmark, and
    monitors for trie-debug artifacts.

  WITH --virgin and --scratch (schelk mode):
    On first run (schelk not initialized), the snapshot is downloaded to the
    virgin volume and schelk is initialized via `init-from`. On subsequent runs,
    `schelk recover` performs a fast surgical restore of scratch from virgin.
    The scratch mount point is used as the reth datadir (--datadir is ignored).

  In all modes the script compiles reth (--profile=profiling -F trie-debug),
  starts `reth node` with --engine.proof-jitter=100us and
  --engine.state-root-task-compare-updates, runs reth-bench for N blocks, and
  monitors for trie-debug artifacts (trie_debug_block_*.json). It exits once
  either reth-bench completes or an artifact is produced.

PREREQUISITES:
  1. minio client (mc) configured with the "minio" alias pointing at the
     snapshot bucket. On dev-brian this is pre-configured:

       mc ls minio/reth-snapshots/

  2. Rust toolchain with the "profiling" profile available (nightly recommended):

       rustup show   # verify toolchain

  3. Run from the root of a reth checkout (cargo workspace root).

  For schelk mode only (--virgin/--scratch):

  4. Two block devices of equal size for virgin and scratch volumes.
     WARNING: both devices will be formatted/overwritten on first run.

  5. Kernel modules for dm-era and the ramdisk driver:

       sudo modprobe dm-era
       sudo modprobe brd rd_nr=1 rd_size=1048576   # 1GB ramdisk

  6. thin-provisioning-tools (provides era_invalidate, needed by schelk):

       sudo apt-get install -y thin-provisioning-tools

  7. schelk must be installed and on PATH:

       which schelk   # should print a path

EXAMPLES:
  # Simple mode -- downloads snapshot into default datadir, runs 10k blocks:
  ./scripts/trie-debug-run.sh

  # Simple mode -- custom datadir, 5k blocks:
  ./scripts/trie-debug-run.sh --datadir /data/trie-debug -n 5000

  # Schelk mode -- fast restore, 10k blocks:
  ./scripts/trie-debug-run.sh \
      --virgin /dev/nvme3n1p1 \
      --scratch /dev/nvme3n1p2

  # Skip snapshot download, reuse existing datadir:
  ./scripts/trie-debug-run.sh --skip-sync -n 10

  # Pass extra args to reth node:
  ./scripts/trie-debug-run.sh -- --log.stdout.filter debug
EOF
    exit 0
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        -n|--blocks)
            [[ -n "${2:-}" ]] || { echo "error: --blocks requires a value"; exit 1; }
            ADVANCE_BLOCKS="$2"
            shift 2
            ;;
        --datadir)
            [[ -n "${2:-}" ]] || { echo "error: --datadir requires a value"; exit 1; }
            DATADIR="$2"
            shift 2
            ;;
        --virgin)
            [[ -n "${2:-}" ]] || { echo "error: --virgin requires a value"; exit 1; }
            VIRGIN_DEV="$2"
            shift 2
            ;;
        --scratch)
            [[ -n "${2:-}" ]] || { echo "error: --scratch requires a value"; exit 1; }
            SCRATCH_DEV="$2"
            shift 2
            ;;
        --mount-point)
            [[ -n "${2:-}" ]] || { echo "error: --mount-point requires a value"; exit 1; }
            MOUNT_POINT="$2"
            shift 2
            ;;
        --ramdisk)
            [[ -n "${2:-}" ]] || { echo "error: --ramdisk requires a value"; exit 1; }
            RAMDISK="$2"
            shift 2
            ;;
        --skip-sync)
            SKIP_SYNC=true
            shift
            ;;
        -h|--help)
            usage
            ;;
        --)
            shift
            EXTRA_RETH_ARGS=("$@")
            break
            ;;
        *)
            echo "Unknown option: $1 (use -- to pass extra args to reth node)"
            exit 1
            ;;
    esac
done

# Determine mode
USE_SCHELK=false
if [[ -n "$VIRGIN_DEV" && -n "$SCRATCH_DEV" ]]; then
    USE_SCHELK=true
elif [[ -n "$VIRGIN_DEV" || -n "$SCRATCH_DEV" ]]; then
    echo "error: --virgin and --scratch must both be provided for schelk mode"
    exit 1
fi

# In schelk mode the datadir is the mount point
if $USE_SCHELK; then
    DATADIR="$MOUNT_POINT"
fi

# -- Configuration ------------------------------------------------------------
MINIO_ALIAS="minio"
MINIO_BUCKET="reth-snapshots"
MAINNET_DATADIR="$HOME/.local/share/reth/mainnet"
JWT_SECRET="$MAINNET_DATADIR/jwt.hex"
LOG_FILE="reth-node-trie-debug.log"
ARTIFACT_PATTERN="trie_debug_block_*.json"
ARCHIVE_DIR="trie-debug-artifacts-archive"
POLL_INTERVAL=10  # seconds

# Ports offset from defaults to avoid conflict with any running mainnet node
AUTHRPC_PORT=9551
HTTP_PORT=9545
WS_PORT=9546
METRICS_PORT=9901
DISCOVERY_PORT=30304

# -- Helpers ------------------------------------------------------------------
log() { echo "[$(date '+%H:%M:%S')] $*" >&2; }
die() { log "FATAL: $*"; exit 1; }

cleanup() {
    log "Cleaning up..."
    if [[ -n "${RETH_PID:-}" ]] && kill -0 "$RETH_PID" 2>/dev/null; then
        log "Sending SIGTERM to reth node (PID $RETH_PID)..."
        kill "$RETH_PID" 2>/dev/null || true
        wait "$RETH_PID" 2>/dev/null || true
        log "reth node stopped."
    fi
    if [[ -n "${BENCH_PID:-}" ]] && kill -0 "$BENCH_PID" 2>/dev/null; then
        log "Killing reth-bench (PID $BENCH_PID)..."
        kill "$BENCH_PID" 2>/dev/null || true
        wait "$BENCH_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# -- Snapshot helper ----------------------------------------------------------
find_oldest_snapshot() {
    log "Querying minio for available weekly snapshots..."
    local snapshot
    snapshot=$(mc ls "${MINIO_ALIAS}/${MINIO_BUCKET}/" \
        | grep 'reth-1-minimal-nightly-weekly-.*\.tar\.zst$' \
        | sort \
        | head -1 \
        | awk '{print $NF}')

    if [[ -z "$snapshot" ]]; then
        die "No weekly snapshots found in ${MINIO_ALIAS}/${MINIO_BUCKET}"
    fi
    log "Oldest snapshot: $snapshot"
    echo "$snapshot"
}

ensure_jwt_secret() {
    local dir="$1"
    if [[ ! -f "$dir/jwt.hex" ]]; then
        if [[ -f "$JWT_SECRET" ]]; then
            cp "$JWT_SECRET" "$dir/jwt.hex"
            log "Copied JWT secret from mainnet datadir."
        else
            openssl rand -hex 32 > "$dir/jwt.hex"
            log "Generated new JWT secret."
        fi
    fi
}

# -- Step 1: Prepare datadir -------------------------------------------------
if $SKIP_SYNC; then
    log "Skipping sync (--skip-sync), using existing datadir: $DATADIR"
    if [[ ! -d "$DATADIR/db" ]]; then
        die "Datadir at $DATADIR/db does not exist. Run without --skip-sync first."
    fi
    ensure_jwt_secret "$DATADIR"
elif $USE_SCHELK; then
    # -- Schelk mode ------------------------------------------------------

    # Check whether the virgin volume already contains reth data by temporarily
    # mounting it. If empty, download the oldest available snapshot.
    populate_virgin() {
        VIRGIN_TMP=$(mktemp -d)

        if sudo mount "$VIRGIN_DEV" "$VIRGIN_TMP" 2>/dev/null && [[ -d "$VIRGIN_TMP/db" ]]; then
            log "Virgin volume already has reth data, skipping download."
            sudo umount "$VIRGIN_TMP"
            rmdir "$VIRGIN_TMP"
            return 0
        fi
        sudo umount "$VIRGIN_TMP" 2>/dev/null || true

        log "Virgin volume has no reth data, downloading snapshot..."

        # Create fresh filesystem
        log "Creating ext4 filesystem on virgin device $VIRGIN_DEV..."
        sudo mkfs.ext4 -F "$VIRGIN_DEV"
        sudo mount "$VIRGIN_DEV" "$VIRGIN_TMP"

        OLDEST_SNAPSHOT=$(find_oldest_snapshot)

        log "Downloading and extracting snapshot to virgin volume (this will take a while)..."
        mc cat "${MINIO_ALIAS}/${MINIO_BUCKET}/${OLDEST_SNAPSHOT}" \
            | zstd -d \
            | sudo tar xf - -C "$VIRGIN_TMP"

        # Ensure JWT secret exists
        if [[ ! -f "$VIRGIN_TMP/jwt.hex" ]]; then
            if [[ -f "$JWT_SECRET" ]]; then
                sudo cp "$JWT_SECRET" "$VIRGIN_TMP/jwt.hex"
                log "Copied JWT secret from mainnet datadir to virgin."
            else
                openssl rand -hex 32 | sudo tee "$VIRGIN_TMP/jwt.hex" > /dev/null
                log "Generated new JWT secret on virgin volume."
            fi
        fi

        log "Snapshot extracted to virgin volume."
        sudo umount "$VIRGIN_TMP"
        rmdir "$VIRGIN_TMP"
        return 1  # signal that virgin was freshly populated
    }

    SCHELK_INITIALIZED=false
    if sudo schelk status > /dev/null 2>&1; then
        SCHELK_INITIALIZED=true
    fi

    if $SCHELK_INITIALIZED; then
        log "schelk is initialized."

        if populate_virgin; then
            VIRGIN_FRESH=false
        else
            VIRGIN_FRESH=true
        fi

        if $VIRGIN_FRESH; then
            log "Virgin was freshly populated, running full-recover..."
            sudo schelk full-recover -y
        else
            log "Recovering scratch from virgin..."
            if ! sudo schelk recover --kill -y 2>/dev/null; then
                log "Surgical recover failed (likely no prior mount), falling back to full-recover..."
                sudo schelk full-recover -y
            fi
        fi

        sudo schelk mount
        log "Scratch restored and mounted at $MOUNT_POINT"
    else
        log "schelk is not initialized, setting up..."

        populate_virgin || true

        log "Initializing schelk (virgin=$VIRGIN_DEV, scratch=$SCRATCH_DEV, mount=$MOUNT_POINT)..."
        sudo mkdir -p "$MOUNT_POINT"
        sudo schelk init-from \
            --virgin "$VIRGIN_DEV" \
            --scratch "$SCRATCH_DEV" \
            --ramdisk "$RAMDISK" \
            --mount-point "$MOUNT_POINT" \
            --fstype ext4 \
            -y
        sudo schelk mount
        log "schelk initialized and scratch mounted at $MOUNT_POINT"
    fi

    # Verify scratch has data after recovery
    if [[ ! -d "$DATADIR/db" ]]; then
        die "Datadir at $DATADIR/db does not exist after schelk setup. Check schelk mount point."
    fi

    # Ensure JWT secret exists in the working datadir
    if [[ ! -f "$DATADIR/jwt.hex" ]]; then
        if [[ -f "$JWT_SECRET" ]]; then
            sudo cp "$JWT_SECRET" "$DATADIR/jwt.hex"
            log "Copied JWT secret from mainnet datadir."
        else
            openssl rand -hex 32 | sudo tee "$DATADIR/jwt.hex" > /dev/null
            log "Generated new JWT secret."
        fi
    fi
else
    # -- Simple datadir mode ----------------------------------------------
    log "Using simple datadir mode: $DATADIR"

    # Wipe and re-download snapshot
    if [[ -d "$DATADIR" ]]; then
        log "Removing existing datadir at $DATADIR..."
        rm -rf "$DATADIR"
    fi
    mkdir -p "$DATADIR"

    OLDEST_SNAPSHOT=$(find_oldest_snapshot)

    log "Downloading and extracting snapshot to $DATADIR (this will take a while)..."
    mc cat "${MINIO_ALIAS}/${MINIO_BUCKET}/${OLDEST_SNAPSHOT}" \
        | zstd -d \
        | tar xf - -C "$DATADIR"

    log "Snapshot extracted."

    ensure_jwt_secret "$DATADIR"

    if [[ ! -d "$DATADIR/db" ]]; then
        die "Datadir at $DATADIR/db does not exist after snapshot extraction."
    fi
fi

# -- Step 2: Compile reth -----------------------------------------------------
log "Compiling reth with --profile=profiling -F trie-debug..."
cargo build --profile=profiling -F trie-debug -p reth
log "Compilation complete."

RETH_BIN="./target/profiling/reth"
[[ -x "$RETH_BIN" ]] || die "reth binary not found at $RETH_BIN"

command -v reth-bench > /dev/null 2>&1 || die "reth-bench not found in PATH"

# -- Step 3: Archive pre-existing trie-debug artifacts ------------------------
if compgen -G "$ARTIFACT_PATTERN" > /dev/null 2>&1; then
    mkdir -p "$ARCHIVE_DIR"
    log "Archiving pre-existing trie-debug artifacts to $ARCHIVE_DIR/..."
    mv $ARTIFACT_PATTERN "$ARCHIVE_DIR/"
fi

# -- Step 4: Start reth node -------------------------------------------------
# Build the reth node argument list
RETH_NODE_ARGS=(
    --datadir "$DATADIR"
    --authrpc.port "$AUTHRPC_PORT"
    --http
    --http.port "$HTTP_PORT"
    --ws
    --ws.port "$WS_PORT"
    --metrics "0.0.0.0:$METRICS_PORT"
    --port "$DISCOVERY_PORT"
    --discovery.port "$DISCOVERY_PORT"
    --engine.state-root-task-compare-updates
    --engine.accept-execution-requests-hash
)

# Add --engine.proof-jitter if the binary supports it
if $RETH_BIN node --help 2>&1 | grep -q 'engine.proof-jitter'; then
    RETH_NODE_ARGS+=(--engine.proof-jitter=100us)
    log "proof-jitter flag available, adding --engine.proof-jitter=100us"
else
    log "proof-jitter flag not available in this build, skipping"
fi

log "Starting reth node..."
log "  Datadir:    $DATADIR"
log "  Extra args: ${EXTRA_RETH_ARGS[*]:-<none>}"
$RETH_BIN node \
    "${RETH_NODE_ARGS[@]}" \
    "${EXTRA_RETH_ARGS[@]}" \
    > "$LOG_FILE" 2>&1 &
RETH_PID=$!
log "reth node started with PID $RETH_PID, logs -> $LOG_FILE"

# Wait for the engine RPC to become ready
log "Waiting for reth engine RPC to be ready on port $AUTHRPC_PORT..."
MAX_WAIT=300
WAITED=0
while ! curl -sf -X POST -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","method":"eth_blockNumber","id":1}' \
    "http://localhost:$HTTP_PORT" > /dev/null 2>&1; do
    if ! kill -0 "$RETH_PID" 2>/dev/null; then
        die "reth node exited before becoming ready. Check $LOG_FILE"
    fi
    sleep 2
    WAITED=$((WAITED + 2))
    if [[ $WAITED -ge $MAX_WAIT ]]; then
        die "Timed out waiting for reth node to become ready after ${MAX_WAIT}s"
    fi
done
log "reth node is ready (waited ${WAITED}s)."

# -- Step 5: Start reth-bench ------------------------------------------------
log "Starting reth-bench new-payload-fcu --advance $ADVANCE_BLOCKS..."
reth-bench new-payload-fcu \
    --rpc-url "https://reth-ethereum.ithaca.xyz/rpc" \
    --engine-rpc-url "http://localhost:$AUTHRPC_PORT" \
    --jwt-secret "$DATADIR/jwt.hex" \
    --ws-rpc-url "ws://localhost:$WS_PORT" \
    --advance "$ADVANCE_BLOCKS" \
    > reth-bench-trie-debug.log 2>&1 &
BENCH_PID=$!
log "reth-bench started with PID $BENCH_PID, logs -> reth-bench-trie-debug.log"

# -- Step 6: Wait for bench completion or trie-debug artifact -----------------
log "Monitoring for reth-bench completion or trie-debug artifact..."
while true; do
    # Check if a trie-debug artifact appeared
    if compgen -G "$ARTIFACT_PATTERN" > /dev/null 2>&1; then
        ARTIFACT=$(ls -1 $ARTIFACT_PATTERN | head -1)
        log "*** Trie-debug artifact detected: $ARTIFACT ***"
        break
    fi

    # Check if reth-bench has finished
    if ! kill -0 "$BENCH_PID" 2>/dev/null; then
        wait "$BENCH_PID" && BENCH_EXIT=0 || BENCH_EXIT=$?
        log "reth-bench exited with code $BENCH_EXIT."
        BENCH_PID=""
        break
    fi

    # Check if reth node has crashed
    if ! kill -0 "$RETH_PID" 2>/dev/null; then
        log "WARNING: reth node exited unexpectedly. Check $LOG_FILE"
        break
    fi

    sleep "$POLL_INTERVAL"
done

# -- Step 7: Shut down reth node ---------------------------------------------
# Kill bench if still running
if [[ -n "${BENCH_PID:-}" ]] && kill -0 "$BENCH_PID" 2>/dev/null; then
    log "Stopping reth-bench (PID $BENCH_PID)..."
    kill "$BENCH_PID" 2>/dev/null || true
    wait "$BENCH_PID" 2>/dev/null || true
    BENCH_PID=""
fi

if kill -0 "$RETH_PID" 2>/dev/null; then
    log "Sending SIGTERM to reth node (PID $RETH_PID)..."
    kill "$RETH_PID"
    log "Waiting for reth node to shut down..."
    wait "$RETH_PID" 2>/dev/null || true
    log "reth node shut down."
fi
RETH_PID=""

# -- Summary ------------------------------------------------------------------
log "=== Run complete ==="
log "  Mode:       $(if $USE_SCHELK; then echo "schelk"; else echo "simple datadir"; fi)"
log "  Datadir:    $DATADIR"
log "  Node logs:  $LOG_FILE"
log "  Bench logs: reth-bench-trie-debug.log"
if compgen -G "$ARTIFACT_PATTERN" > /dev/null 2>&1; then
    log "  Artifacts:  $(ls -1 $ARTIFACT_PATTERN | tr '\n' ' ')"
else
    log "  No trie-debug artifacts were produced."
fi
