#!/usr/bin/env bash
set -euo pipefail

for f in /sys/devices/system/cpu/cpu*/cpufreq/scaling_min_freq; do
  cat "$(dirname "$f")/cpuinfo_min_freq" | sudo tee "$f" > /dev/null 2>&1 || true
done
for f in /sys/devices/system/cpu/cpu*/cpufreq/scaling_max_freq; do
  cat "$(dirname "$f")/cpuinfo_max_freq" | sudo tee "$f" > /dev/null 2>&1 || true
done

echo active | sudo tee /sys/devices/system/cpu/amd_pstate/status 2>/dev/null || true
if [ -n "${BENCH_CPU_DMA_LATENCY_PID:-}" ]; then
  sudo kill "$BENCH_CPU_DMA_LATENCY_PID" 2>/dev/null || true
fi
sudo pkill -f '^bench-cpu-dma-latency' 2>/dev/null || true
sudo cpupower frequency-set -g powersave 2>/dev/null || true
sudo systemctl start irqbalance cron atd 2>/dev/null || true
