/**
 * Paradigm Reth ExEx & Benchmark Unit Tests
 */

import { defaultExExSimulator } from '../src/core/exex-simulator.js';
import { defaultBenchmarkEngine } from '../src/core/benchmark-engine.js';

async function runExExTests() {
  console.log('Testing Paradigm Reth ExEx Pipeline & Performance Engine...');

  // 1. ExEx Block Commit
  const evt = defaultExExSimulator.processExExBlock();
  if (!evt.blockHash || !evt.executionSpeed) {
    throw new Error('ExEx block stream processing failed');
  }

  // 2. Benchmark Simulation
  const bm = defaultBenchmarkEngine.runBenchmark();
  if (!bm.metrics.blockExecutionThroughput || !bm.storageEngine.includes('MDBX')) {
    throw new Error('Reth MDBX benchmark execution failed');
  }

  console.log(`✅ Paradigm Reth ExEx Stream & MDBX Benchmark Tested (#${evt.blockNumber} @ ${evt.executionSpeed})!`);
}

runExExTests().catch(e => {
  console.error('❌ ExEx Test Failed:', e);
  process.exit(1);
});
