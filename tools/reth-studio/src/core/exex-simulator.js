/**
 * Paradigm Reth ExEx (Execution Extensions) Simulator
 */

import crypto from 'crypto';
import { RETH_CONFIG } from '../config.js';

export class RethExExSimulator {
  constructor() {
    this.currentBlockNumber = 20584100;
    this.exexEvents = [];
    this.activePipelines = RETH_CONFIG.exexPipelines;
  }

  /**
   * Process a new block through the Reth ExEx pipeline
   */
  processExExBlock() {
    this.currentBlockNumber += 1;
    const blockHash = '0x' + crypto.randomBytes(32).toString('hex');
    const stateRoot = '0x' + crypto.randomBytes(32).toString('hex');
    const txCount = Math.floor(Math.random() * 120) + 80;
    const gasUsed = (Math.random() * 8 + 12).toFixed(2) + ' Mgas';

    const event = {
      id: `exex_${Date.now()}`,
      blockNumber: this.currentBlockNumber,
      blockHash,
      stateRoot,
      txCount,
      gasUsed,
      executionSpeed: `${(Math.random() * 400 + 5800).toFixed(0)} Mgas/s`,
      latencyMs: (Math.random() * 1.5 + 0.8).toFixed(2),
      timestamp: new Date().toISOString(),
      status: 'COMMITTED_IN_MDBX',
    };

    this.exexEvents.unshift(event);
    return event;
  }

  getRecentEvents() {
    return this.exexEvents.slice(0, 10);
  }
}

export const defaultExExSimulator = new RethExExSimulator();
