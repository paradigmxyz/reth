import type { BlockHeader } from './header'
import type { Common } from '@nomicfoundation/ethereumjs-common'
import type {
  AccessListEIP2930TxData,
  FeeMarketEIP1559TxData,
  JsonRpcTx,
  JsonTx,
  TxData,
} from '@nomicfoundation/ethereumjs-tx'
import type { AddressLike, BigIntLike, BufferLike } from '@nomicfoundation/ethereumjs-util'
/**
 * An object to set to which blockchain the blocks and their headers belong. This could be specified
 * using a {@link Common} object, or `chain` and `hardfork`. Defaults to mainnet without specifying a
 * hardfork.
 */
export interface BlockOptions {
  /**
   * A {@link Common} object defining the chain and the hardfork a block/block header belongs to.
   *
   * Object will be internally copied so that tx behavior don't incidentally
   * change on future HF changes.
   *
   * Default: {@link Common} object set to `mainnet` and the HF currently defined as the default
   * hardfork in the {@link Common} class.
   *
   * Current default hardfork: `merge`
   */
  common?: Common
  /**
   * Determine the HF by the block number
   *
   * Default: `false` (HF is set to whatever default HF is set by the {@link Common} instance)
   */
  hardforkByBlockNumber?: boolean
  /**
   * Determine the HF by total difficulty (Merge HF)
   *
   * This option is a superset of `hardforkByBlockNumber` (so only use one of both options)
   * and determines the HF by both the block number and the TD.
   *
   * Since the TTD is only a threshold the block number will in doubt take precedence (imagine
   * e.g. both Merge and Shanghai HF blocks set and the block number from the block provided
   * pointing to a Shanghai block: this will lead to set the HF as Shanghai and not the Merge).
   */
  hardforkByTTD?: BigIntLike
  /**
   * If a preceding {@link BlockHeader} (usually the parent header) is given the preceding
   * header will be used to calculate the difficulty for this block and the calculated
   * difficulty takes precedence over a provided static `difficulty` value.
   *
   * Note that this option has no effect on networks other than PoW/Ethash networks
   * (respectively also deactivates on the Merge HF switching to PoS/Casper).
   */
  calcDifficultyFromHeader?: BlockHeader
  /**
   * A block object by default gets frozen along initialization. This gives you
   * strong additional security guarantees on the consistency of the block parameters.
   * It also enables block hash caching when the `hash()` method is called multiple times.
   *
   * If you need to deactivate the block freeze - e.g. because you want to subclass block and
   * add aditional properties - it is strongly encouraged that you do the freeze yourself
   * within your code instead.
   *
   * Default: true
   */
  freeze?: boolean
  /**
   * Provide a clique signer's privateKey to seal this block.
   * Will throw if provided on a non-PoA chain.
   */
  cliqueSigner?: Buffer
  /**
   *  Skip consensus format validation checks on header if set. Defaults to false.
   */
  skipConsensusFormatValidation?: boolean
}

/**
 * A block header's data.
 */
export interface HeaderData {
  parentHash?: BufferLike
  uncleHash?: BufferLike
  coinbase?: AddressLike
  stateRoot?: BufferLike
  transactionsTrie?: BufferLike
  receiptTrie?: BufferLike
  logsBloom?: BufferLike
  difficulty?: BigIntLike
  number?: BigIntLike
  gasLimit?: BigIntLike
  gasUsed?: BigIntLike
  timestamp?: BigIntLike
  extraData?: BufferLike
  mixHash?: BufferLike
  nonce?: BufferLike
  baseFeePerGas?: BigIntLike
}

/**
 * A block's data.
 */
export interface BlockData {
  /**
   * Header data for the block
   */
  header?: HeaderData
  transactions?: Array<TxData | AccessListEIP2930TxData | FeeMarketEIP1559TxData>
  uncleHeaders?: Array<HeaderData>
}

export type BlockBuffer = [BlockHeaderBuffer, TransactionsBuffer, UncleHeadersBuffer]
export type BlockHeaderBuffer = Buffer[]
export type BlockBodyBuffer = [TransactionsBuffer, UncleHeadersBuffer]
/**
 * TransactionsBuffer can be an array of serialized txs for Typed Transactions or an array of Buffer Arrays for legacy transactions.
 */
export type TransactionsBuffer = Buffer[][] | Buffer[]
export type UncleHeadersBuffer = Buffer[][]

/**
 * An object with the block's data represented as strings.
 */
export interface JsonBlock {
  /**
   * Header data for the block
   */
  header?: JsonHeader
  transactions?: JsonTx[]
  uncleHeaders?: JsonHeader[]
}

/**
 * An object with the block header's data represented as strings.
 */
export interface JsonHeader {
  parentHash?: string
  uncleHash?: string
  coinbase?: string
  stateRoot?: string
  transactionsTrie?: string
  receiptTrie?: string
  logsBloom?: string
  difficulty?: string
  number?: string
  gasLimit?: string
  gasUsed?: string
  timestamp?: string
  extraData?: string
  mixHash?: string
  nonce?: string
  baseFeePerGas?: string
}

/*
 * Based on https://eth.wiki/json-rpc/API
 */
export interface JsonRpcBlock {
  number: string // the block number. null when pending block.
  hash: string // hash of the block. null when pending block.
  parentHash: string // hash of the parent block.
  mixHash?: string // bit hash which proves combined with the nonce that a sufficient amount of computation has been carried out on this block.
  nonce: string // hash of the generated proof-of-work. null when pending block.
  sha3Uncles: string // SHA3 of the uncles data in the block.
  logsBloom: string // the bloom filter for the logs of the block. null when pending block.
  transactionsRoot: string // the root of the transaction trie of the block.
  stateRoot: string // the root of the final state trie of the block.
  receiptsRoot: string // the root of the receipts trie of the block.
  miner: string // the address of the beneficiary to whom the mining rewards were given.
  difficulty: string // integer of the difficulty for this block.
  totalDifficulty: string // integer of the total difficulty of the chain until this block.
  extraData: string // the “extra data” field of this block.
  size: string // integer the size of this block in bytes.
  gasLimit: string // the maximum gas allowed in this block.
  gasUsed: string // the total used gas by all transactions in this block.
  timestamp: string // the unix timestamp for when the block was collated.
  transactions: Array<JsonRpcTx | string> // Array of transaction objects, or 32 Bytes transaction hashes depending on the last given parameter.
  uncles: string[] // Array of uncle hashes
  baseFeePerGas?: string // If EIP-1559 is enabled for this block, returns the base fee per gas
}
