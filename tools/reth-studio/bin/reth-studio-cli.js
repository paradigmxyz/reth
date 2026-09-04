#!/usr/bin/env node

/**
 * Paradigm Reth Studio CLI
 */

import { defaultExExSimulator } from '../src/core/exex-simulator.js';
import { defaultBenchmarkEngine } from '../src/core/benchmark-engine.js';

const args = process.argv.slice(2);
const command = args[0] || 'help';

async function main() {
  switch (command.toLowerCase()) {
    case 'benchmark': {
      console.log('\n⚡ Running Paradigm Reth EVM & MDBX Benchmark:');
      const bm = defaultBenchmarkEngine.runBenchmark();
      console.log(`  Engine:              ${bm.storageEngine}`);
      console.log(`  Read Latency:        ${bm.metrics.randomAccountReadLatency}`);
      console.log(`  Write Latency:       ${bm.metrics.sequentialSlotWriteLatency}`);
      console.log(`  Execution Speed:     ${bm.metrics.blockExecutionThroughput}`);
      console.log(`  Sync Throughput:     ${bm.metrics.historicalSyncThroughput}`);
      console.log(`  Memory Footprint:    ${bm.metrics.memoryFootprint}\n`);
      break;
    }

    case 'exex': {
      console.log('\n🚀 Triggering Reth ExEx Block Stream Tick...');
      const event = defaultExExSimulator.processExExBlock();
      console.log(`  Block Number:    #${event.blockNumber}`);
      console.log(`  Transactions:    ${event.txCount}`);
      console.log(`  Gas Used:        ${event.gasUsed}`);
      console.log(`  Execution Speed: ${event.executionSpeed}`);
      console.log(`  Latency:         ${event.latencyMs} ms`);
      console.log(`  Status:          ${event.status}\n`);
      break;
    }

    case 'studio': {
      console.log('\n🌐 Launching Reth Studio on :3415...');
      await import('../src/server/app.js');
      break;
    }

    default: {
      console.log(`
╔══════════════════════════════════════════════════════════════════╗
║               ⚡ PARADIGM RETH EXECUTION CLI                     ║
║         Rust Ethereum Execution Extensions & Benchmarks          ║
╚══════════════════════════════════════════════════════════════════╝

Commands:
  reth-studio-cli benchmark            Run MDBX storage & execution benchmarks
  reth-studio-cli exex                 Stream real-time block via ExEx engine
  reth-studio-cli studio               Launch Interactive Web Studio on :3415
      `);
      break;
    }
  }
}

main().catch(err => {
  console.error('Error:', err.message);
  process.exit(1);
});
