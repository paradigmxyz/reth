/**
 * Paradigm Reth Studio Client Logic
 */

let isAutoStreaming = false;
let autoInterval = null;

document.addEventListener('DOMContentLoaded', () => {
  initTabs();
  loadConfig();
  loadBenchmark();
  initListeners();
});

function initTabs() {
  const tabs = document.querySelectorAll('.nav-tab');
  tabs.forEach(tab => {
    tab.addEventListener('click', () => {
      document.querySelectorAll('.nav-tab').forEach(t => t.classList.toggle('active', t === tab));
      document.querySelectorAll('.tab-pane').forEach(p => p.classList.toggle('active', p.id === `tab-${tab.dataset.tab}`));
    });
  });
}

async function loadConfig() {
  try {
    const res = await fetch('/api/config');
    const data = await res.json();
    document.getElementById('header-speed').textContent = `> ${data.benchmarks.syncSpeedGigaGasPerSec} Ggas/s`;
  } catch (e) {
    console.error(e);
  }
}

async function loadBenchmark() {
  try {
    const res = await fetch('/api/benchmark');
    const data = await res.json();
    document.getElementById('bm-read').textContent = data.metrics.randomAccountReadLatency;
    document.getElementById('bm-throughput').textContent = data.metrics.blockExecutionThroughput;
    document.getElementById('bm-sync').textContent = data.metrics.historicalSyncThroughput;
  } catch (e) {
    console.error(e);
  }
}

function initListeners() {
  document.getElementById('btn-trigger-block').addEventListener('click', processBlock);
  document.getElementById('btn-refresh-bm').addEventListener('click', loadBenchmark);

  const autoBtn = document.getElementById('btn-toggle-auto');
  autoBtn.addEventListener('click', () => {
    if (isAutoStreaming) {
      clearInterval(autoInterval);
      isAutoStreaming = false;
      autoBtn.textContent = '▶️ Start Auto-Block Stream (1 block/sec)';
      autoBtn.className = 'btn btn-gradient btn-lg';
    } else {
      isAutoStreaming = true;
      autoBtn.textContent = '⏸️ Pause ExEx Stream';
      autoBtn.className = 'btn btn-secondary btn-lg';
      processBlock();
      autoInterval = setInterval(processBlock, 1000);
    }
  });
}

async function processBlock() {
  try {
    const res = await fetch('/api/exex/tick', { method: 'POST' });
    const data = await res.json();
    if (data.success) {
      appendBlockRow(data.event);
    }
  } catch (e) {
    console.warn(e);
  }
}

function appendBlockRow(evt) {
  const container = document.getElementById('blocks-container');
  const empty = container.querySelector('.empty-state');
  if (empty) container.innerHTML = '';

  const row = document.createElement('div');
  row.className = 'ledger-row';
  row.innerHTML = `
    <div>
      <div style="font-weight: 700; color: #fff;">Block #${evt.blockNumber.toLocaleString()}</div>
      <div class="mono text-muted" style="font-size: 0.72rem;">StateRoot: ${evt.stateRoot.slice(0, 16)}...</div>
    </div>
    <div style="text-align: right;">
      <div style="color: #34d399; font-weight: 700; font-family: var(--font-mono);">${evt.executionSpeed}</div>
      <div class="mono text-muted" style="font-size: 0.72rem;">${evt.txCount} txs • ${evt.latencyMs}ms</div>
    </div>
  `;
  container.insertBefore(row, container.firstChild);
}
