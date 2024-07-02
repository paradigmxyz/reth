import { ConsensusAlgorithm } from '@nomicfoundation/ethereumjs-common'

import type { Consensus } from './interface'
import type { BlockHeader } from '@nomicfoundation/ethereumjs-block'

/**
 * This class encapsulates Casper-related consensus functionality when used with the Blockchain class.
 */
export class CasperConsensus implements Consensus {
  algorithm: ConsensusAlgorithm

  constructor() {
    this.algorithm = ConsensusAlgorithm.Casper
  }

  public async genesisInit(): Promise<void> {}

  public async setup(): Promise<void> {}

  public async validateConsensus(): Promise<void> {}

  public async validateDifficulty(header: BlockHeader): Promise<void> {
    if (header.difficulty !== BigInt(0)) {
      const msg = 'invalid difficulty.  PoS blocks must have difficulty 0'
      throw new Error(`${msg} ${header.errorStr()}`)
    }
  }
  public async newBlock(): Promise<void> {}
}
