#!/usr/bin/env bash
#
# local-reth-bench.sh — Run the reth Engine API benchmark locally.
#
# Replicates the CI bench.yml workflow (build, snapshot, system tuning,
# interleaved B-F-F-B execution, summary, charts) without any GitHub
# Actions glue (no PR comments, no artifact upload, no Slack).
#
# Usage:
#   local-reth-bench.sh <baseline-ref> <feature-ref> [options]
#
# Options:
#   --blocks N      Number of blocks to benchmark (default: 500)
#   --warmup N      Number of warmup blocks (default: 100)
#   --cores N       Limit reth to N CPU cores, 0 = all available (default: 0)
#   --samply        Enable samply profiling
#   --tracy MODE    Tracy profiling: off, on, full (default: off)
#   --tracy-filter F Tracy tracing filter (default: debug)
#   --no-tune       Skip system tuning (useful on dev machines / macOS)
#
# Requires: the reth repo at RETH_REPO (default: ~/reth)
#
# Dependencies (install before first run):
#   mc (MinIO client), schelk, cpupower, taskset, stdbuf, python3, curl,
#   make, uv, pzstd, jq, Rust toolchain (cargo/rustup)
#
# The script delegates to the existing bench-reth-*.sh scripts in the reth
# repo for the actual build, snapshot, and run steps.
set -euxo pipefail

# ── PATH ──────────────────────────────────────────────────────────────
# Ensure cargo and user-local bins (mc, uv) are visible
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"

# ── Defaults ──────────────────────────────────────────────────────────
RETH_REPO="${RETH_REPO:-$HOME/reth}"
BLOCKS=500
WARMUP=100
CORES=0
SAMPLY=false
TRACY="off"
TRACY_FILTER="debug"
TUNE=true
BASELINE_REF=""
FEATURE_REF=""

# ── Parse arguments ──────────────────────────────────────────────────
usage() {
  cat <<EOF
Usage: $(basename "$0") <baseline-ref> <feature-ref> [options]

Options:
  --blocks N         Number of blocks to benchmark (default: 500)
  --warmup N         Number of warmup blocks (default: 100)
  --cores N          Limit reth to N CPU cores (default: 0 = all)
  --samply           Enable samply profiling
  --tracy MODE       Tracy profiling: off, on, full (default: off)
                       on   = tracing only (lower overhead)
                       full = tracing + CPU sampling (higher overhead)
  --tracy-filter F   Tracy tracing filter (default: debug)
  --no-tune          Skip system tuning
EOF
  exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --blocks)       BLOCKS="$2"; shift 2 ;;
    --warmup)       WARMUP="$2"; shift 2 ;;
    --cores)        CORES="$2"; shift 2 ;;
    --samply)       SAMPLY=true; shift ;;
    --tracy)        TRACY="$2"; shift 2 ;;
    --tracy-filter) TRACY_FILTER="$2"; shift 2 ;;
    --no-tune)      TUNE=false; shift ;;
    --help|-h)  usage ;;
    -*)         echo "Unknown option: $1"; usage ;;
    *)
      if [ -z "$BASELINE_REF" ]; then
        BASELINE_REF="$1"
      elif [ -z "$FEATURE_REF" ]; then
        FEATURE_REF="$1"
      else
        echo "Unexpected argument: $1"; usage
      fi
      shift
      ;;
  esac
done

if [ -z "$BASELINE_REF" ] || [ -z "$FEATURE_REF" ]; then
  echo "Error: both <baseline-ref> and <feature-ref> are required."
  usage
fi

# Validate --tracy value
case "$TRACY" in
  off|on|full) ;;
  *) echo "Error: --tracy must be off, on, or full (got: $TRACY)"; usage ;;
esac

# Samply + tracy=full are mutually exclusive (both use perf sampling)
if [ "$SAMPLY" = "true" ] && [ "$TRACY" = "full" ]; then
  echo "Warning: samply and tracy=full both use perf sampling; downgrading tracy to 'on'."
  TRACY="on"
fi

# ── Check dependencies ───────────────────────────────────────────────
missing=()
for cmd in mc schelk cpupower taskset stdbuf python3 curl make uv pzstd jq cargo; do
  command -v "$cmd" &>/dev/null || missing+=("$cmd")
done
if [ ${#missing[@]} -gt 0 ]; then
  echo "Error: missing required tools: ${missing[*]}"
  echo "See the CI 'Install dependencies' step in .github/workflows/bench.yml for install instructions."
  exit 1
fi

if [ "$TRACY" != "off" ]; then
  if ! command -v tracy-capture &>/dev/null; then
    echo "Error: tracy-capture is required for --tracy $TRACY"
    exit 1
  fi
fi

# Ensure tools that run via sudo are in a sudo-visible path.
# The bench scripts use `sudo schelk` / `sudo samply` but cargo installs
# them to ~/.cargo/bin which sudo's secure_path doesn't include.
for cmd in schelk samply; do
  if command -v "$cmd" &>/dev/null && ! sudo sh -c "command -v $cmd" &>/dev/null; then
    echo "Installing $cmd to /usr/local/bin (needed for sudo)..."
    sudo install "$(command -v "$cmd")" /usr/local/bin/
  fi
done

if [ ! -d "$RETH_REPO/.git" ]; then
  echo "Error: RETH_REPO=$RETH_REPO is not a git repository."
  echo "Set RETH_REPO or clone reth to ~/reth"
  exit 1
fi

# ── Resolve paths ────────────────────────────────────────────────────
SELF_DIR="$(cd "$(dirname "$0")" && pwd)"
SCRIPTS_DIR="${RETH_REPO}/.github/scripts"
BENCH_WORK_DIR="${RETH_REPO}/../bench-work-$(date +%Y%m%d-%H%M%S)"
BASELINE_SRC="${RETH_REPO}/../reth-baseline"
FEATURE_SRC="${RETH_REPO}/../reth-feature"

mkdir -p "$BENCH_WORK_DIR"
BENCH_WORK_DIR="$(cd "$BENCH_WORK_DIR" && pwd)"

# ── Global cleanup trap (restores system tuning on any exit) ─────────
TUNING_APPLIED=false
CSTATE_PID=
METRICS_PROXY_PID=
cleanup_global() {
  [ -n "$METRICS_PROXY_PID" ] && kill "$METRICS_PROXY_PID" 2>/dev/null || true
  if [ "$TUNING_APPLIED" = true ]; then
    echo
    echo "▸ Restoring system settings..."
    [ -n "$CSTATE_PID" ] && kill "$CSTATE_PID" 2>/dev/null || true
    sudo systemctl start irqbalance cron atd 2>/dev/null || true
    echo "  System settings restored."
  fi
}
trap cleanup_global EXIT

echo "═══════════════════════════════════════════════════════════"
echo "  reth local benchmark"
echo "═══════════════════════════════════════════════════════════"
echo "  Baseline ref : $BASELINE_REF"
echo "  Feature ref  : $FEATURE_REF"
echo "  Blocks       : $BLOCKS"
echo "  Warmup       : $WARMUP"
echo "  Cores        : $CORES"
echo "  Samply       : $SAMPLY"
echo "  Tracy        : $TRACY"
echo "  Tracy filter : $TRACY_FILTER"
echo "  System tune  : $TUNE"
echo "  Work dir     : $BENCH_WORK_DIR"
echo "  Reth repo    : $RETH_REPO"
echo "═══════════════════════════════════════════════════════════"
echo

# Enable sccache if available (matches CI's RUSTC_WRAPPER=sccache)
if command -v sccache &>/dev/null; then
  export RUSTC_WRAPPER="sccache"
fi

# Export env vars expected by the bench-reth-*.sh scripts
export BENCH_BLOCKS="$BLOCKS"
export BENCH_WARMUP_BLOCKS="$WARMUP"
export BENCH_CORES="$CORES"
export BENCH_SAMPLY="$SAMPLY"
export BENCH_TRACY="$TRACY"
export BENCH_TRACY_FILTER="$TRACY_FILTER"
export BENCH_WORK_DIR
export SCHELK_MOUNT="${SCHELK_MOUNT:-/reth-bench}"
export BENCH_RPC_URL="${BENCH_RPC_URL:-https://ethereum.reth.rs/rpc}"
export BENCH_METRICS_ADDR="127.0.0.1:9100"

# ── Step 1: Resolve refs to full SHAs ────────────────────────────────
echo "▸ Resolving git refs..."
cd "$RETH_REPO"

resolve_ref() {
  local ref="$1"
  git fetch origin "$ref" --quiet 2>/dev/null || true
  git rev-parse "$ref" 2>/dev/null \
    || git rev-parse "origin/$ref" 2>/dev/null \
    || { echo "Error: cannot resolve ref '$ref'"; exit 1; }
}

BASELINE_SHA="$(resolve_ref "$BASELINE_REF")"
FEATURE_SHA="$(resolve_ref "$FEATURE_REF")"
echo "  Baseline SHA : $BASELINE_SHA"
echo "  Feature SHA  : $FEATURE_SHA"
echo

# ── Step 2: Prepare source directories ───────────────────────────────
echo "▸ Preparing source directories..."

prepare_source() {
  local src_dir="$1" ref="$2"
  if [ -d "$src_dir" ]; then
    git -C "$src_dir" fetch origin "$ref" 2>/dev/null || true
  else
    git clone --recurse-submodules "$RETH_REPO" "$src_dir"
  fi
  git -C "$src_dir" checkout "$ref" --force
  git -C "$src_dir" submodule update --init --recursive
}

prepare_source "$BASELINE_SRC" "$BASELINE_SHA"
prepare_source "$FEATURE_SRC" "$FEATURE_SHA"
BASELINE_SRC="$(cd "$BASELINE_SRC" && pwd)"
FEATURE_SRC="$(cd "$FEATURE_SRC" && pwd)"
echo "  Baseline src : $BASELINE_SRC"
echo "  Feature src  : $FEATURE_SRC"
echo

# ── Step 3: Check / download snapshot ────────────────────────────────
echo "▸ Checking snapshot..."
cd "$RETH_REPO"
SNAPSHOT_NEEDED=false
if ! "${SCRIPTS_DIR}/bench-reth-snapshot.sh" --check; then
  SNAPSHOT_NEEDED=true
  echo "  Snapshot needs update."
else
  echo "  Snapshot is up-to-date."
fi
echo

# ── Step 4: Build binaries (+ snapshot download) in parallel ─────────
echo "▸ Building binaries (parallel)..."
cd "$RETH_REPO"

FAIL=0

"${SCRIPTS_DIR}/bench-reth-build.sh" baseline "$BASELINE_SRC" "$BASELINE_SHA" &
PID_BASELINE=$!

"${SCRIPTS_DIR}/bench-reth-build.sh" feature "$FEATURE_SRC" "$FEATURE_SHA" &
PID_FEATURE=$!

PID_SNAPSHOT=
if [ "$SNAPSHOT_NEEDED" = "true" ]; then
  echo "  Also downloading snapshot in parallel..."
  "${SCRIPTS_DIR}/bench-reth-snapshot.sh" &
  PID_SNAPSHOT=$!
fi

wait $PID_BASELINE || FAIL=1
wait $PID_FEATURE  || FAIL=1
[ -n "$PID_SNAPSHOT" ] && { wait $PID_SNAPSHOT || FAIL=1; }

if [ $FAIL -ne 0 ]; then
  echo "Error: one or more parallel tasks failed (builds / snapshot)"
  exit 1
fi
echo "  Binaries built successfully."
echo

# ── Step 5: System tuning (optional) ────────────────────────────────
if [ "$TUNE" = "true" ]; then
  echo "▸ Applying system tuning..."

  sudo cpupower frequency-set -g performance 2>/dev/null || true

  # Disable turbo boost (Intel + AMD)
  echo 1 | sudo tee /sys/devices/system/cpu/intel_pstate/no_turbo 2>/dev/null || true
  echo 0 | sudo tee /sys/devices/system/cpu/cpufreq/boost 2>/dev/null || true

  sudo swapoff -a 2>/dev/null || true
  echo 0 | sudo tee /proc/sys/kernel/randomize_va_space 2>/dev/null || true

  # Disable SMT (hyperthreading)
  for cpu in /sys/devices/system/cpu/cpu*/topology/thread_siblings_list; do
    [ -f "$cpu" ] || continue
    first=$(cut -d, -f1 < "$cpu" | cut -d- -f1)
    current=$(echo "$cpu" | grep -o 'cpu[0-9]*' | grep -o '[0-9]*')
    if [ "$current" != "$first" ]; then
      echo 0 | sudo tee "/sys/devices/system/cpu/cpu${current}/online" 2>/dev/null || true
    fi
  done
  echo "  Online CPUs: $(nproc)"

  # Disable transparent huge pages
  for p in /sys/kernel/mm/transparent_hugepage /sys/kernel/mm/transparent_hugepages; do
    if [ -d "$p" ]; then
      echo never | sudo tee "$p/enabled" 2>/dev/null || true
      echo never | sudo tee "$p/defrag" 2>/dev/null || true
      break
    fi
  done

  # Prevent deep C-states
  sudo sh -c 'exec 3<>/dev/cpu_dma_latency; echo -ne "\x00\x00\x00\x00" >&3; sleep infinity' &
  CSTATE_PID=$!

  # Pin IRQs to core 0
  for irq in /proc/irq/*/smp_affinity_list; do
    echo 0 | sudo tee "$irq" 2>/dev/null || true
  done

  # Stop noisy background services
  sudo systemctl stop irqbalance cron atd unattended-upgrades snapd 2>/dev/null || true

  TUNING_APPLIED=true

  # Log environment for reproducibility (matches CI)
  echo "  === Benchmark environment ==="
  echo "  Kernel : $(uname -r)"
  lscpu | grep -E 'Model name|CPU\(s\)|MHz|NUMA' | sed 's/^/  /'
  echo "  Governor : $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null || echo unknown)"
  echo "  Freq     : $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq 2>/dev/null || echo unknown)"
  echo "  THP      : $(cat /sys/kernel/mm/transparent_hugepage/enabled 2>/dev/null || cat /sys/kernel/mm/transparent_hugepages/enabled 2>/dev/null || echo unknown)"
  free -h | sed 's/^/  /'
  echo "  System tuning applied."
  echo
fi

# ── Step 5b: Tracefs mount (tracy=full only) ─────────────────────────
if [ "$TRACY" = "full" ] && [ "$(uname)" = "Linux" ]; then
  echo "▸ Mounting tracefs for Tracy full mode..."
  sudo mount -t tracefs tracefs /sys/kernel/tracing -o mode=755 2>/dev/null || true
fi

# ── Tracy upload & viewer helpers ────────────────────────────────────
TRACY_VIEWER_BASE="${TRACY_VIEWER_BASE:-}"

tracy_viewer_url() {
  local profile_url="$1"
  if [ -z "$TRACY_VIEWER_BASE" ]; then
    echo ""
    return
  fi
  local encoded
  encoded=$(python3 -c "import urllib.parse, sys; print(urllib.parse.quote(sys.argv[1], safe=''))" "$profile_url")
  echo "${TRACY_VIEWER_BASE}?profile_url=${encoded}"
}

upload_tracy() {
  local label="$1" output_dir="$2" sha="$3"
  local tracy_file="$output_dir/tracy-profile.tracy"

  if [ ! -f "$tracy_file" ]; then
    echo "  Tracy: no profile found, skipping upload."
    return
  fi

  local timestamp short_sha remote_name bucket mc_alias
  timestamp=$(date +%Y%m%d-%H%M%S)
  short_sha="${sha:0:7}"
  remote_name="${label}-${short_sha}-${timestamp}.tracy"
  bucket="${TRACY_BUCKET:-tracy-profiles}"
  mc_alias="${MC_ALIAS:-minio}"
  local minio_base="${TRACY_MINIO_URL:-http://minio.minio.svc.cluster.local:9000}"

  echo "  Tracy: uploading profile..."
  if mc cp "$tracy_file" "${mc_alias}/${bucket}/${remote_name}"; then
    local url="${minio_base}/${bucket}/${remote_name}"
    echo "$url" > "$output_dir/tracy_url.txt"
    local viewer
    viewer=$(tracy_viewer_url "$url")
    if [ -n "$viewer" ]; then
      echo "$viewer" > "$output_dir/tracy_viewer_url.txt"
      echo "  Tracy: uploaded → $viewer"
    else
      echo "  Tracy: uploaded → $url"
    fi
  else
    echo "  Tracy: upload failed (non-fatal)."
  fi

  # Delete large profile to free disk
  rm -f "$tracy_file"
}

# ── Step 6: Pre-flight cleanup ───────────────────────────────────────
echo "▸ Pre-flight cleanup..."
pkill -f bench-metrics-proxy 2>/dev/null || true
sudo systemctl stop "${RETH_SCOPE:-reth-bench.scope}" 2>/dev/null || true
sudo systemctl reset-failed "${RETH_SCOPE:-reth-bench.scope}" 2>/dev/null || true
sudo schelk recover -y --kill || sudo schelk full-recover -y || true
echo

# ── Step 7: Interleaved benchmark runs (B-F-F-B) ────────────────────
# This ordering reduces systematic bias from thermal drift and cache warming.
BASELINE_BIN="${BASELINE_SRC}/target/profiling/reth"
FEATURE_BIN="${FEATURE_SRC}/target/profiling/reth"

# Start metrics proxy (reth → label injection → Prometheus)
LABELS_FILE="/tmp/bench-metrics-labels.json"
echo '{}' > "$LABELS_FILE"
METRICS_SUBNET="${METRICS_SUBNET:-10.10.0.0/24}"
METRICS_PORT="${METRICS_PORT:-9090}"
python3 "${SELF_DIR}/bench-metrics-proxy.py" \
  --labels "$LABELS_FILE" \
  --upstream "http://${BENCH_METRICS_ADDR}/" \
  --subnet "$METRICS_SUBNET" \
  --port "$METRICS_PORT" &
METRICS_PROXY_PID=$!
echo "▸ Metrics proxy started (PID $METRICS_PROXY_PID) on subnet ${METRICS_SUBNET}, port ${METRICS_PORT}"

# Unique benchmark ID: local-<timestamp> for local runs, ci-<run_id> for CI
BENCH_ID="local-$(basename "$BENCH_WORK_DIR" | sed 's/bench-work-//')"
# Reference epoch: shared time origin so all runs overlay in Grafana.
# The proxy maps each run's elapsed time onto this common origin.
BENCH_REFERENCE_EPOCH=$(date +%s)

write_labels() {
  local run_label="$1" run_type="$2" ref="$3" sha="$4"
  LAST_RUN_START=$(date +%s)
  cat > "$LABELS_FILE" <<-EOF
	{"benchmark_run":"${run_label}","run_type":"${run_type}","git_ref":"${ref}","bench_sha":"${sha}","benchmark_id":"${BENCH_ID}","run_start_epoch":"${LAST_RUN_START}","reference_epoch":"${BENCH_REFERENCE_EPOCH}"}
	EOF
}

run_bench() {
  local label="$1" binary="$2" output_dir="$3"
  echo "▸ Running benchmark: ${label}..."
  cd "$RETH_REPO"
  if command -v taskset &>/dev/null; then
    taskset -c 0 "${SCRIPTS_DIR}/bench-reth-run.sh" "$label" "$binary" "$output_dir"
  else
    "${SCRIPTS_DIR}/bench-reth-run.sh" "$label" "$binary" "$output_dir"
  fi
  echo "  ✓ ${label} complete."
  echo
}

write_labels "baseline-1" "baseline" "$BASELINE_REF" "$BASELINE_SHA"
run_bench "baseline-1" "$BASELINE_BIN" "$BENCH_WORK_DIR/baseline-1"

write_labels "feature-1" "feature" "$FEATURE_REF" "$FEATURE_SHA"
run_bench "feature-1"  "$FEATURE_BIN"  "$BENCH_WORK_DIR/feature-1"

write_labels "feature-2" "feature" "$FEATURE_REF" "$FEATURE_SHA"
run_bench "feature-2"  "$FEATURE_BIN"  "$BENCH_WORK_DIR/feature-2"

write_labels "baseline-2" "baseline" "$BASELINE_REF" "$BASELINE_SHA"
run_bench "baseline-2" "$BASELINE_BIN" "$BENCH_WORK_DIR/baseline-2"

# ── Compute Grafana URL ──────────────────────────────────────────────
GRAFANA_BASE_URL="https://tempoxyz.grafana.net/d/reth-bench-ghr/reth-bench-ghr"
GRAFANA_DATASOURCE="ef57fux92e9z4e"
LAST_RUN_DURATION=$(( $(date +%s) - LAST_RUN_START ))
FROM_MS=$(( BENCH_REFERENCE_EPOCH * 1000 ))
TO_MS=$(( (BENCH_REFERENCE_EPOCH + LAST_RUN_DURATION) * 1000 ))
GRAFANA_URL="${GRAFANA_BASE_URL}?orgId=1&from=${FROM_MS}&to=${TO_MS}&timezone=browser&var-datasource=${GRAFANA_DATASOURCE}&var-job=reth-bench&var-benchmark_id=${BENCH_ID}&var-benchmark_run=\$__all"

# ── Step 8: Scan logs for errors ─────────────────────────────────────
echo "▸ Scanning logs for errors..."
ERRORS_FILE="$BENCH_WORK_DIR/errors.md"
found_errors=false
for run_dir in baseline-1 feature-1 feature-2 baseline-2; do
  LOG="$BENCH_WORK_DIR/$run_dir/node.log"
  [ -f "$LOG" ] || continue
  panics=$(grep -c -E 'panicked at' "$LOG" 2>/dev/null || true)
  errors=$(grep -c ' ERROR ' "$LOG" 2>/dev/null || true)
  if [ "$panics" -gt 0 ] || [ "$errors" -gt 0 ]; then
    if [ "$found_errors" = false ]; then
      printf '### ⚠️ Node Errors\n\n' >> "$ERRORS_FILE"
      found_errors=true
    fi
    printf '<details><summary><b>%s</b>: %d panic(s), %d error(s)</summary>\n\n' \
      "$run_dir" "$panics" "$errors" >> "$ERRORS_FILE"
    if [ "$panics" -gt 0 ]; then
      printf '**Panics:**\n```\n' >> "$ERRORS_FILE"
      grep -E 'panicked at' "$LOG" | head -10 >> "$ERRORS_FILE"
      printf '```\n' >> "$ERRORS_FILE"
    fi
    if [ "$errors" -gt 0 ]; then
      printf '**Errors (first 20):**\n```\n' >> "$ERRORS_FILE"
      grep ' ERROR ' "$LOG" | head -20 >> "$ERRORS_FILE"
      printf '```\n' >> "$ERRORS_FILE"
    fi
    printf '\n</details>\n\n' >> "$ERRORS_FILE"
  fi
done
if [ "$found_errors" = true ]; then
  echo "  ⚠ Errors found — see $ERRORS_FILE"
else
  echo "  No errors found."
fi
echo

# ── Step 9: Parse results ───────────────────────────────────────────
echo "▸ Parsing results..."
cd "$RETH_REPO"

SUMMARY_ARGS=(
  --output-summary "$BENCH_WORK_DIR/summary.json"
  --output-markdown "$BENCH_WORK_DIR/comment.md"
  --repo "paradigmxyz/reth"
  --baseline-ref "$BASELINE_SHA"
  --baseline-name "$BASELINE_REF"
  --feature-name "$FEATURE_REF"
  --feature-ref "$FEATURE_SHA"
  --baseline-csv "$BENCH_WORK_DIR/baseline-1/combined_latency.csv" "$BENCH_WORK_DIR/baseline-2/combined_latency.csv"
  --feature-csv "$BENCH_WORK_DIR/feature-1/combined_latency.csv" "$BENCH_WORK_DIR/feature-2/combined_latency.csv"
  --gas-csv "$BENCH_WORK_DIR/feature-1/total_gas.csv"
  --grafana-url "$GRAFANA_URL"
)

python3 "${SCRIPTS_DIR}/bench-reth-summary.py" "${SUMMARY_ARGS[@]}"
echo

# ── Step 10: Generate charts ─────────────────────────────────────────
echo "▸ Generating charts..."
CHART_ARGS=(
  --output-dir "$BENCH_WORK_DIR/charts"
  --feature "$BENCH_WORK_DIR/feature-1/combined_latency.csv" "$BENCH_WORK_DIR/feature-2/combined_latency.csv"
  --baseline "$BENCH_WORK_DIR/baseline-1/combined_latency.csv" "$BENCH_WORK_DIR/baseline-2/combined_latency.csv"
  --baseline-name "$BASELINE_REF"
  --feature-name "$FEATURE_REF"
)

if python3 -c "import matplotlib" 2>/dev/null; then
  python3 "${SCRIPTS_DIR}/bench-reth-charts.py" "${CHART_ARGS[@]}"
elif command -v uv &>/dev/null; then
  uv run --with matplotlib python3 "${SCRIPTS_DIR}/bench-reth-charts.py" "${CHART_ARGS[@]}"
else
  echo "  Warning: matplotlib not available, skipping chart generation."
fi
echo

# ── Step 11: Upload Tracy profiles ────────────────────────────────────
if [ "$TRACY" != "off" ]; then
  echo "▸ Uploading Tracy profiles..."
  upload_tracy "baseline-1" "$BENCH_WORK_DIR/baseline-1" "$BASELINE_SHA"
  upload_tracy "feature-1"  "$BENCH_WORK_DIR/feature-1"  "$FEATURE_SHA"
  upload_tracy "feature-2"  "$BENCH_WORK_DIR/feature-2"  "$FEATURE_SHA"
  upload_tracy "baseline-2" "$BENCH_WORK_DIR/baseline-2" "$BASELINE_SHA"
  echo
fi

# ── Done (system restore happens via EXIT trap) ─────────────────────
echo "═══════════════════════════════════════════════════════════"
echo "  Benchmark complete!"
echo "═══════════════════════════════════════════════════════════"
echo "  Results  : $BENCH_WORK_DIR/summary.json"
echo "  Markdown : $BENCH_WORK_DIR/comment.md"
echo "  Charts   : $BENCH_WORK_DIR/charts/"
if [ -f "$ERRORS_FILE" ]; then
  echo "  Errors   : $ERRORS_FILE"
fi
echo "  Grafana  : $GRAFANA_URL"
if [ "$TRACY" != "off" ]; then
  echo "  ─── Tracy Profiles ───"
  for run_dir in baseline-1 feature-1 feature-2 baseline-2; do
    url_file="$BENCH_WORK_DIR/$run_dir/tracy_viewer_url.txt"
    if [ -f "$url_file" ]; then
      echo "  $run_dir : $(cat "$url_file")"
    fi
  done
fi
echo "═══════════════════════════════════════════════════════════"
