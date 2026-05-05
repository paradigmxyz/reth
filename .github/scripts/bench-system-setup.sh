#!/usr/bin/env bash
set -euo pipefail

# Switch amd-pstate to passive mode so the kernel governor controls frequency directly.
echo passive | sudo tee /sys/devices/system/cpu/amd_pstate/status 2>/dev/null || true
sudo cpupower frequency-set -g performance || true

NOMINAL_KHZ=""
if [ -f /sys/devices/system/cpu/cpu0/acpi_cppc/nominal_freq ]; then
  NOMINAL_MHZ=$(cat /sys/devices/system/cpu/cpu0/acpi_cppc/nominal_freq)
  NOMINAL_KHZ=$((NOMINAL_MHZ * 1000))
elif [ -f /sys/devices/system/cpu/cpu0/cpufreq/base_frequency ]; then
  NOMINAL_KHZ=$(cat /sys/devices/system/cpu/cpu0/cpufreq/base_frequency)
fi
if [ -n "$NOMINAL_KHZ" ] && [ "$NOMINAL_KHZ" -gt 0 ] 2>/dev/null; then
  echo "Pinning all cores to nominal frequency: $((NOMINAL_KHZ / 1000)) MHz"
  for f in /sys/devices/system/cpu/cpu*/cpufreq/scaling_max_freq; do
    echo "$NOMINAL_KHZ" | sudo tee "$f" > /dev/null
  done
  for f in /sys/devices/system/cpu/cpu*/cpufreq/scaling_min_freq; do
    echo "$NOMINAL_KHZ" | sudo tee "$f" > /dev/null
  done
fi

sudo swapoff -a || true
echo 0 | sudo tee /proc/sys/kernel/randomize_va_space || true

for cpu in /sys/devices/system/cpu/cpu*/topology/thread_siblings_list; do
  first=$(cut -d, -f1 < "$cpu" | cut -d- -f1)
  current=$(echo "$cpu" | grep -o 'cpu[0-9]*' | grep -o '[0-9]*')
  if [ "$current" != "$first" ]; then
    echo 0 | sudo tee "/sys/devices/system/cpu/cpu${current}/online" || true
  fi
done

echo "Online CPUs: $(nproc)"
for p in /sys/kernel/mm/transparent_hugepage /sys/kernel/mm/transparent_hugepages; do
  [ -d "$p" ] && echo never | sudo tee "$p/enabled" && echo never | sudo tee "$p/defrag" && break
done || true

sudo pkill -f '^bench-cpu-dma-latency' 2>/dev/null || true
sudo bash -c 'exec 3<>/dev/cpu_dma_latency; printf "\0\0\0\0" >&3; exec -a bench-cpu-dma-latency sleep infinity' &
echo "BENCH_CPU_DMA_LATENCY_PID=$!" >> "$GITHUB_ENV"

for irq in /proc/irq/*/smp_affinity_list; do
  echo 0 | sudo tee "$irq" 2>/dev/null || true
done

sudo systemctl stop irqbalance cron atd unattended-upgrades snapd 2>/dev/null || true

echo "=== Benchmark environment ==="
uname -r
lscpu | grep -E 'Model name|CPU\(s\)|MHz|NUMA'
cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor
cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq
echo "scaling_min_freq: $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_min_freq)"
echo "scaling_max_freq: $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_max_freq)"
cat /sys/kernel/mm/transparent_hugepage/enabled 2>/dev/null || cat /sys/kernel/mm/transparent_hugepages/enabled 2>/dev/null || echo "THP: unknown"
free -h
