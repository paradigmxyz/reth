import type { Consensus } from './consensus'
import type { GenesisState } from './genesisStates'
import type { Block, BlockHeader } from '@nomicfoundation/ethereumjs-block'
import type { Common } from '@nomicfoundation/ethereumjs-common'
import type { AbstractLevel } from 'abstract-level'

export type OnBlock = (block: Block, reorg: boolean) => Promise<void> | void

export interface BlockchainInterface {
  consensus: Consensus
  /**
   * Adds a block to the blockchain.
   *
   * @param block - The block to be added to the blockchain.
   */
  putBlock(block: Block): Promise<void>

  /**
   * Deletes a block from the blockchain. All child blocks in the chain are
   * deleted and any encountered heads are set to the parent block.
   *
   * @param blockHash - The hash of the block to be deleted
   */
  delBlock(blockHash: Buffer): Promise<void>

  /**
   * Returns a block by its hash or number.
   */
  getBlock(blockId: Buffer | number | bigint): Promise<Block>

  /**
   * Iterates through blocks starting at the specified iterator head and calls
   * the onBlock function on each block.
   *
   * @param name - Name of the state root head
   * @param onBlock - Function called on each block with params (block: Block,
   * @param maxBlocks - optional maximum number of blocks to iterate through
   * reorg: boolean)
   */
  iterator(
    name: string,
    onBlock: OnBlock,
    maxBlocks?: number,
    releaseLockOnCallback?: boolean
  ): Promise<number>

  /**
   * Returns a copy of the blockchain
   */
  copy(): BlockchainInterface

  /**
   * Validates a block header, throwing if invalid. It is being validated against the reported `parentHash`.
   * @param header - header to be validated
   * @param height - If this is an uncle header, this is the height of the block that is including it
   */
  validateHeader(header: BlockHeader, height?: bigint): Promise<void>

  /**
   * Returns the specified iterator head.
   *
   * @param name - Optional name of the iterator head (default: 'vm')
   */
  getIteratorHead?(name?: string): Promise<Block>

  /**
   * Gets total difficulty for a block specified by hash and number
   */
  getTotalDifficulty?(hash: Buffer, number?: bigint): Promise<bigint>

  /**
   * Returns the genesis state of the blockchain.
   * All values are provided as hex-prefixed strings.
   */
  genesisState?(): GenesisState

  /**
   * Returns the latest full block in the canonical chain.
   */
  getCanonicalHeadBlock?(): Promise<Block>
}

/**
 * This are the options that the Blockchain constructor can receive.
 */
export interface BlockchainOptions {
  /**
   * Specify the chain and hardfork by passing a {@link Common} instance.
   *
   * If not provided this defaults to chain `mainnet` and hardfork `chainstart`
   *
   */
  common?: Common

  /**
   * Set the HF to the fork determined by the head block and update on head updates.
   *
   * Note: for HFs where the transition is also determined by a total difficulty
   * threshold (merge HF) the calculated TD is additionally taken into account
   * for HF determination.
   *
   * Default: `false` (HF is set to whatever default HF is set by the {@link Common} instance)
   */
  hardforkByHeadBlockNumber?: boolean

  /**
   * Database to store blocks and metadata.
   * Should be an `abstract-leveldown` compliant store
   * wrapped with `encoding-down`.
   * For example:
   *   `levelup(encode(leveldown('./db1')))`
   * or use the `level` convenience package:
   *   `new MemoryLevel('./db1')`
   */
  db?: AbstractLevel<string | Buffer | Uint8Array, string | Buffer, string | Buffer>

  /**
   * This flags indicates if a block should be validated along the consensus algorithm
   * or protocol used by the chain, e.g. by verifying the PoW on the block.
   *
   * Supported consensus types and algorithms (taken from the `Common` instance):
   * - 'pow' with 'ethash' algorithm (validates the proof-of-work)
   * - 'poa' with 'clique' algorithm (verifies the block signatures)
   * Default: `true`.
   */
  validateConsensus?: boolean

  /**
   * This flag indicates if protocol-given consistency checks on
   * block headers and included uncles and transactions should be performed,
   * see Block#validate for details.
   *
   */
  validateBlocks?: boolean

  /**
   * The blockchain only initializes successfully if it has a genesis block. If
   * there is no block available in the DB and a `genesisBlock` is provided,
   * then the provided `genesisBlock` will be used as genesis. If no block is
   * present in the DB and no block is provided, then the genesis block as
   * provided from the `common` will be used.
   */
  genesisBlock?: Block

  /**
   * If you are using a custom chain {@link Common}, pass the genesis state.
   *
   * Pattern 1 (with genesis state see {@link GenesisState} for format):
   *
   * ```javascript
   * {
   *   '0x0...01': '0x100', // For EoA
   * }
   * ```
   *
   * Pattern 2 (with complex genesis state, containing contract accounts and storage).
   * Note that in {@link AccountState} there are two
   * accepted types. This allows to easily insert accounts in the genesis state:
   *
   * A complex genesis state with Contract and EoA states would have the following format:
   *
   * ```javascript
   * {
   *   '0x0...01': '0x100', // For EoA
   *   '0x0...02': ['0x1', '0xRUNTIME_BYTECODE', [[storageKey1, storageValue1], [storageKey2, storageValue2]]] // For contracts
   * }
   * ```
   */
  genesisState?: GenesisState

  /**
   * Optional custom consensus that implements the {@link Consensus} class
   */
  consensus?: Consensus
}
