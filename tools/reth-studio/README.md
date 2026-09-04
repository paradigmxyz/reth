# ⚡ Paradigm Reth Studio & ExEx Suite

An execution inspector, **Execution Extension (ExEx) Pipeline Simulator**, and **MDBX Storage Benchmark Dashboard** for **Paradigm Reth (Rust Ethereum)**.

---

## 🌟 Key Features

- ⚡ **Paradigm ExEx Pipeline**: Stream committed blocks, state notifications, and indexer rollups with zero-copy memory overhead.
- 🚀 **MDBX Storage Benchmarks**: Measure real-time read/write latencies and block execution throughput (> 6,000 Mgas/s).
- 🌐 **Interactive Web Studio**: Real-time ExEx streaming dashboard and node metrics visualizer on `http://localhost:3415`.
- ⌨️ **Universal CLI (`reth-studio-cli`)**: Terminal utility for streaming ExEx blocks and triggering performance benchmarks.

---

## 🚀 Quickstart

```bash
# Launch Reth Studio
npm start
# Open http://localhost:3415

# Or run via CLI
node bin/reth-studio-cli.js exex
node bin/reth-studio-cli.js benchmark
```
