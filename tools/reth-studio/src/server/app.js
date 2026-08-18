/**
 * Paradigm Reth Studio Web Server
 */

import express from 'express';
import cors from 'cors';
import path from 'path';
import { fileURLToPath } from 'url';
import { RETH_CONFIG } from '../config.js';
import { defaultExExSimulator } from '../core/exex-simulator.js';
import { defaultBenchmarkEngine } from '../core/benchmark-engine.js';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const WEB_ROOT = path.join(__dirname, '../../web');

const app = express();
const PORT = process.env.PORT || 3415;

app.use(cors());
app.use(express.json());
app.use(express.static(WEB_ROOT));

// 1. Get Client Config & ExEx Pipelines
app.get('/api/config', (req, res) => {
  res.json({
    client: RETH_CONFIG.client,
    pipelines: RETH_CONFIG.exexPipelines,
    benchmarks: RETH_CONFIG.benchmarks,
  });
});

// 2. Trigger ExEx Block Process
app.post('/api/exex/tick', (req, res) => {
  const event = defaultExExSimulator.processExExBlock();
  res.json({ success: true, event });
});

// 3. ExEx Stream History
app.get('/api/exex/events', (req, res) => {
  res.json(defaultExExSimulator.getRecentEvents());
});

// 4. Run Live Performance Benchmark
app.get('/api/benchmark', (req, res) => {
  res.json(defaultBenchmarkEngine.runBenchmark());
});

if (process.env.NODE_ENV !== 'test') {
  app.listen(PORT, () => {
    console.log(`\n======================================================`);
    console.log(`⚡ Paradigm Reth Execution & ExEx Studio Running!`);
    console.log(`🌐 Web Dashboard: http://localhost:${PORT}`);
    console.log(`🚀 Storage Engine: MDBX Zero-Copy Memory Mapped DB`);
    console.log(`======================================================\n`);
  });
}

export default app;
