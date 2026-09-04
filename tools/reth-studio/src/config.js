/**
 * Paradigm Reth Architecture & ExEx Configuration
 */

export const RETH_CONFIG = {
  client: {
    name: 'Reth (Rust Ethereum)',
    developer: 'Paradigm',
    language: 'Rust',
    storageEngine: 'MDBX (Memory-Mapped DB)',
    consensusSupport: 'Engine API / PoS Consensus Clients',
  },
  exexPipelines: [
    {
      id: 'exex_shadow_indexer',
      name: 'Shadow Rollup & Custom Indexer',
      description: 'Zero-overhead streaming of committed blocks directly from memory without JSON-RPC bottleneck.',
      throughput: '6,200 Mgas/s',
      latency: '< 2ms',
    },
    {
      id: 'exex_mev_searcher',
      name: 'Real-time State Diff & MEV Backrunner',
      description: 'Listens to raw state root mutations and pre-execution bundles.',
      throughput: '5,800 Mgas/s',
      latency: '< 1ms',
    },
    {
      id: 'exex_bridge_attestation',
      name: 'Cross-Chain L1/L2 Finality Watcher',
      description: 'Streams finalized block receipts for instant optimistic bridge settlement.',
      throughput: '6,000 Mgas/s',
      latency: '< 3ms',
    },
  ],
  benchmarks: {
    syncSpeedGigaGasPerSec: 2.8,
    dbReadLatencyMicroSec: 12.4,
    dbWriteLatencyMicroSec: 28.6,
    activeExExStreams: 3,
  },
};
