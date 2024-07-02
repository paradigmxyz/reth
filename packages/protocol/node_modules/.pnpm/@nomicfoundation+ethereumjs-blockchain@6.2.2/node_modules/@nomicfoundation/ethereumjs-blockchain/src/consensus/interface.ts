import type { Blockchain } from '..'
import type { Block, BlockHeader } from '@nomicfoundation/ethereumjs-block'
import type { ConsensusAlgorithm } from '@nomicfoundation/ethereumjs-common'

/**
 * Interface that a consensus class needs to implement.
 */
export interface Consensus {
  algorithm: ConsensusAlgorithm | string
  /**
   * Initialize genesis for consensus mechanism
   * @param genesisBlock genesis block
   */
  genesisInit(genesisBlock: Block): Promise<void>

  /**
   * Set up consensus mechanism
   */
  setup({ blockchain }: ConsensusOptions): Promise<void>

  /**
   * Validate block consensus parameters
   * @param block block to be validated
   */
  validateConsensus(block: Block): Promise<void>

  validateDifficulty(header: BlockHeader): Promise<void>

  /**
   * Update consensus on new block
   * @param block new block
   * @param commonAncestor common ancestor block header (optional)
   * @param ancientHeaders array of ancestor block headers (optional)
   */
  newBlock(
    block: Block,
    commonAncestor?: BlockHeader,
    ancientHeaders?: BlockHeader[]
  ): Promise<void>
}

/**
 * Options when initializing a class that implements the Consensus interface.
 */
export interface ConsensusOptions {
  blockchain: Blockchain
}
