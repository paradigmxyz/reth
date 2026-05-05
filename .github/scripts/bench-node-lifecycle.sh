#!/usr/bin/env bash

bench_init_node_lifecycle() {
  LABEL="${1:?label required}"
  BINARY="${2:?binary required}"
  OUTPUT_DIR="${3:?output dir required}"
  DATADIR="${4:?datadir required}"
  mkdir -p "$OUTPUT_DIR"
  LOG="${OUTPUT_DIR}/node.log"
  RETH_SCOPE="${RETH_SCOPE:-reth-bench.scope}"
  TAIL_PID=""
  TRACY_PID=""
}

bench_stop_tracy_capture() {
  if [ -n "${TRACY_PID:-}" ] && kill -0 "$TRACY_PID" 2>/dev/null; then
    echo "Stopping tracy-capture..."
    kill -INT "$TRACY_PID" 2>/dev/null || true
    for i in $(seq 1 30); do
      kill -0 "$TRACY_PID" 2>/dev/null || break
      if [ $((i % 10)) -eq 0 ]; then
        echo "Waiting for tracy-capture to finish writing... (${i}s)"
      fi
      sleep 1
    done
    if kill -0 "$TRACY_PID" 2>/dev/null; then
      echo "tracy-capture still running after 30s, killing..."
      kill -9 "$TRACY_PID" 2>/dev/null || true
    fi
    wait "$TRACY_PID" 2>/dev/null || true
  fi
}

bench_cleanup_node() {
  kill "${TAIL_PID:-}" 2>/dev/null || true
  bench_stop_tracy_capture
  if sudo systemctl is-active "$RETH_SCOPE" >/dev/null 2>&1; then
    if [ "${BENCH_SAMPLY:-false}" = "true" ]; then
      # Send SIGINT to the inner reth process by exact name (not -f which
      # would also match samply's cmdline containing "reth"). Samply will
      # capture reth's exit and save the profile.
      sudo pkill -INT -x reth 2>/dev/null || true
      for i in $(seq 1 120); do
        sudo pgrep -x samply > /dev/null 2>&1 || break
        if [ $((i % 10)) -eq 0 ]; then
          echo "Waiting for samply to finish writing profile... (${i}s)"
        fi
        sleep 1
      done
      if sudo pgrep -x samply > /dev/null 2>&1; then
        echo "Samply still running after 120s, sending SIGTERM..."
        sudo pkill -x samply 2>/dev/null || true
      fi
    fi
    sudo systemctl stop "$RETH_SCOPE" 2>/dev/null || true
    sleep 1
  fi
  sudo systemctl reset-failed "$RETH_SCOPE" 2>/dev/null || true
  sudo chown -R "$(id -un):$(id -gn)" "$OUTPUT_DIR" 2>/dev/null || true
  sudo schelk recover -y --kill || true
}

bench_prepare_snapshot() {
  sudo systemctl stop "$RETH_SCOPE" 2>/dev/null || true
  sudo systemctl reset-failed "$RETH_SCOPE" 2>/dev/null || true
  sudo schelk recover -y --kill || sudo schelk full-recover -y || true

  sudo schelk mount -y || true
  if [ ! -d "$DATADIR/db" ] || [ ! -d "$DATADIR/static_files" ]; then
    echo "::error::Failed to mount benchmark datadir at ${DATADIR}"
    ls -la "$SCHELK_MOUNT" || true
    ls -la "$DATADIR" || true
    exit 1
  fi
  sync
  sudo sh -c 'echo 3 > /proc/sys/vm/drop_caches'
  echo "=== Cache state after drop ==="
  free -h
  grep Cached /proc/meminfo
}

bench_compute_reth_cpus() {
  local online max_reth
  online=$(nproc --all)
  max_reth=$(( online - 1 ))
  if [ "${BENCH_CORES:-0}" -gt 0 ] && [ "$BENCH_CORES" -lt "$max_reth" ]; then
    max_reth=$BENCH_CORES
  fi
  RETH_CPUS="1-${max_reth}"
}

bench_build_reth_args() {
  RETH_ARGS=(
    node
    --datadir "$DATADIR"
    --log.file.directory "$OUTPUT_DIR/reth-logs"
    --engine.accept-execution-requests-hash
    --http
    --http.port 8545
    --ws
    --ws.api all
    --authrpc.port 8551
    --disable-discovery
    --no-persist-peers
  )

  SYNC_STATE_IDLE=false
  if "$BINARY" node --help 2>/dev/null | grep -qF -- '--debug.startup-sync-state-idle'; then
    RETH_ARGS+=(--debug.startup-sync-state-idle)
    SYNC_STATE_IDLE=true
  fi

  EXTRA_NODE_ARGS=""
  case "$LABEL" in
    baseline*) EXTRA_NODE_ARGS="${BENCH_BASELINE_ARGS:-}" ;;
    feature*)  EXTRA_NODE_ARGS="${BENCH_FEATURE_ARGS:-}" ;;
  esac
  if [ -n "$EXTRA_NODE_ARGS" ]; then
    # shellcheck disable=SC2206
    RETH_ARGS+=($EXTRA_NODE_ARGS)
  fi

  if [ -n "${BENCH_METRICS_ADDR:-}" ]; then
    RETH_ARGS+=(--metrics "$BENCH_METRICS_ADDR")
  fi

  if [ "${BENCH_OTLP_DISABLED:-false}" != "true" ]; then
    if [ -n "${BENCH_OTLP_TRACES_ENDPOINT:-}" ]; then
      RETH_ARGS+=(--tracing-otlp="${BENCH_OTLP_TRACES_ENDPOINT}" --tracing-otlp.service-name=reth-bench)
    fi
    if [ -n "${BENCH_OTLP_LOGS_ENDPOINT:-}" ]; then
      RETH_ARGS+=(--logs-otlp="${BENCH_OTLP_LOGS_ENDPOINT}" --logs-otlp.filter=debug)
    fi
  fi

  if [ "${BENCH_TRACY:-off}" != "off" ]; then
    RETH_ARGS+=(--log.tracy --log.tracy.filter "${BENCH_TRACY_FILTER:-debug}")
    if [ "${BENCH_TRACY}" = "on" ]; then
      export TRACY_NO_SYS_TRACE=1
    elif [ "${BENCH_TRACY}" = "full" ]; then
      export TRACY_SAMPLING_HZ="${BENCH_TRACY_SAMPLING_HZ:-1}"
    fi
  fi

  SUDO_ENV=()
  if [ -n "${OTEL_RESOURCE_ATTRIBUTES:-}" ]; then
    SUDO_ENV+=("OTEL_RESOURCE_ATTRIBUTES=${OTEL_RESOURCE_ATTRIBUTES}")
    SUDO_ENV+=("OTEL_BSP_MAX_QUEUE_SIZE=65536" "OTEL_BLRP_MAX_QUEUE_SIZE=65536")
  fi
}

bench_start_reth() {
  local total_mem_kb mem_limit samply
  bench_compute_reth_cpus
  total_mem_kb=$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)
  mem_limit=$(( total_mem_kb * 95 / 100 * 1024 ))
  echo "Memory limit: $(( mem_limit / 1024 / 1024 ))MB (95% of $(( total_mem_kb / 1024 ))MB)"

  if [ "${BENCH_SAMPLY:-false}" = "true" ]; then
    RETH_ARGS+=(--log.samply)
    samply="$(which samply)"
    sudo systemd-run --quiet --scope --collect --unit="$RETH_SCOPE" \
      -p MemoryMax="$mem_limit" -p AllowedCPUs="$RETH_CPUS" \
      env "${SUDO_ENV[@]}" nice -n -20 \
      "$samply" record --save-only --presymbolicate --rate 10000 \
      --output "$OUTPUT_DIR/samply-profile.json.gz" \
      -- "$BINARY" "${RETH_ARGS[@]}" \
      > "$LOG" 2>&1 &
  else
    sudo systemd-run --quiet --scope --collect --unit="$RETH_SCOPE" \
      -p MemoryMax="$mem_limit" -p AllowedCPUs="$RETH_CPUS" \
      env "${SUDO_ENV[@]}" nice -n -20 "$BINARY" "${RETH_ARGS[@]}" \
      > "$LOG" 2>&1 &
  fi
  stdbuf -oL tail -f "$LOG" | sed -u "s/^/[reth] /" &
  TAIL_PID=$!
}

bench_wait_reth_ready() {
  for i in $(seq 1 60); do
    if curl -sf http://127.0.0.1:8545 -X POST \
      -H 'Content-Type: application/json' \
      -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
      > /dev/null 2>&1; then
      echo "reth (${LABEL}) RPC is up after ${i}s"
      break
    fi
    if [ "$i" -eq 60 ]; then
      echo "::error::reth (${LABEL}) failed to start within 60s"
      cat "$LOG"
      exit 1
    fi
    sleep 1
  done

  if [ "$SYNC_STATE_IDLE" = "true" ]; then
    for i in $(seq 1 300); do
      SYNC_RESULT=$(curl -sf http://127.0.0.1:8545 -X POST \
        -H 'Content-Type: application/json' \
        -d '{"jsonrpc":"2.0","method":"eth_syncing","params":[],"id":1}' 2>/dev/null || true)
      if [ -n "$SYNC_RESULT" ] && jq -e '.result == false' <<< "$SYNC_RESULT" > /dev/null 2>&1; then
        echo "reth (${LABEL}) pipeline finished after ${i}s, engine is live"
        break
      fi
      if [ "$i" -eq 300 ]; then
        echo "::error::reth (${LABEL}) pipeline did not finish within 300s"
        cat "$LOG"
        exit 1
      fi
      sleep 1
    done
  else
    echo "reth (${LABEL}) binary does not support --debug.startup-sync-state-idle, skipping sync wait"
  fi
}

bench_start_tracy_capture() {
  if [ "${BENCH_TRACY:-off}" != "off" ]; then
    echo "Starting tracy-capture..."
    tracy-capture -f -o "$OUTPUT_DIR/tracy-profile.tracy" &
    TRACY_PID=$!
    sleep 0.5
  fi
}

bench_run_node_lifecycle() {
  trap bench_cleanup_node EXIT
  bench_prepare_snapshot
  bench_build_reth_args
  bench_start_reth
  bench_wait_reth_ready
}
