import { ConsensusType } from '@nomicfoundation/ethereumjs-common'
import { RLP } from '@nomicfoundation/ethereumjs-rlp'
import { Trie } from '@nomicfoundation/ethereumjs-trie'
import { Capability, TransactionFactory } from '@nomicfoundation/ethereumjs-tx'
import {
  KECCAK256_RLP,
  arrToBufArr,
  bufArrToArr,
  bufferToHex,
} from '@nomicfoundation/ethereumjs-util'
import { keccak256 } from 'ethereum-cryptography/keccak'

import { BlockHeader } from './header'

import type { BlockBuffer, BlockData, BlockOptions, JsonBlock } from './types'
import type { Common } from '@nomicfoundation/ethereumjs-common'
import type {
  FeeMarketEIP1559Transaction,
  Transaction,
  TxOptions,
  TypedTransaction,
} from '@nomicfoundation/ethereumjs-tx'

/**
 * An object that represents the block.
 */
export class Block {
  public readonly header: BlockHeader
  public readonly transactions: TypedTransaction[] = []
  public readonly uncleHeaders: BlockHeader[] = []
  public readonly txTrie = new Trie()
  public readonly _common: Common

  /**
   * Static constructor to create a block from a block data dictionary
   *
   * @param blockData
   * @param opts
   */
  public static fromBlockData(blockData: BlockData = {}, opts?: BlockOptions) {
    const { header: headerData, transactions: txsData, uncleHeaders: uhsData } = blockData
    const header = BlockHeader.fromHeaderData(headerData, opts)

    // parse transactions
    const transactions = []
    for (const txData of txsData ?? []) {
      const tx = TransactionFactory.fromTxData(txData, {
        ...opts,
        // Use header common in case of hardforkByBlockNumber being activated
        common: header._common,
      } as TxOptions)
      transactions.push(tx)
    }

    // parse uncle headers
    const uncleHeaders = []
    const uncleOpts: BlockOptions = {
      hardforkByBlockNumber: true,
      ...opts,
      // Use header common in case of hardforkByBlockNumber being activated
      common: header._common,
      // Disable this option here (all other options carried over), since this overwrites the provided Difficulty to an incorrect value
      calcDifficultyFromHeader: undefined,
      // This potentially overwrites hardforkBy options but we will set them cleanly just below
      hardforkByTTD: undefined,
    }
    // Uncles are obsolete post-merge, any hardfork by option implies hardforkByBlockNumber
    if (opts?.hardforkByTTD !== undefined) {
      uncleOpts.hardforkByBlockNumber = true
    }
    for (const uhData of uhsData ?? []) {
      const uh = BlockHeader.fromHeaderData(uhData, uncleOpts)
      uncleHeaders.push(uh)
    }

    return new Block(header, transactions, uncleHeaders, opts)
  }

  /**
   * Static constructor to create a block from a RLP-serialized block
   *
   * @param serialized
   * @param opts
   */
  public static fromRLPSerializedBlock(serialized: Buffer, opts?: BlockOptions) {
    const values = arrToBufArr(RLP.decode(Uint8Array.from(serialized))) as BlockBuffer

    if (!Array.isArray(values)) {
      throw new Error('Invalid serialized block input. Must be array')
    }

    return Block.fromValuesArray(values, opts)
  }

  /**
   * Static constructor to create a block from an array of Buffer values
   *
   * @param values
   * @param opts
   */
  public static fromValuesArray(values: BlockBuffer, opts?: BlockOptions) {
    if (values.length > 3) {
      throw new Error('invalid block. More values than expected were received')
    }

    const [headerData, txsData, uhsData] = values

    const header = BlockHeader.fromValuesArray(headerData, opts)

    // parse transactions
    const transactions = []
    for (const txData of txsData ?? []) {
      transactions.push(
        TransactionFactory.fromBlockBodyData(txData, {
          ...opts,
          // Use header common in case of hardforkByBlockNumber being activated
          common: header._common,
        })
      )
    }

    // parse uncle headers
    const uncleHeaders = []
    const uncleOpts: BlockOptions = {
      hardforkByBlockNumber: true,
      ...opts,
      // Use header common in case of hardforkByBlockNumber being activated
      common: header._common,
      // Disable this option here (all other options carried over), since this overwrites the provided Difficulty to an incorrect value
      calcDifficultyFromHeader: undefined,
      // This potentially overwrites hardforkBy options but we will set them cleanly just below
      hardforkByTTD: undefined,
    }
    // Uncles are obsolete post-merge, any hardfork by option implies hardforkByBlockNumber
    if (opts?.hardforkByTTD !== undefined) {
      uncleOpts.hardforkByBlockNumber = true
    }
    for (const uncleHeaderData of uhsData ?? []) {
      uncleHeaders.push(BlockHeader.fromValuesArray(uncleHeaderData, uncleOpts))
    }

    return new Block(header, transactions, uncleHeaders, opts)
  }

  /**
   * This constructor takes the values, validates them, assigns them and freezes the object.
   * Use the static factory methods to assist in creating a Block object from varying data types and options.
   */
  constructor(
    header?: BlockHeader,
    transactions: TypedTransaction[] = [],
    uncleHeaders: BlockHeader[] = [],
    opts: BlockOptions = {}
  ) {
    this.header = header ?? BlockHeader.fromHeaderData({}, opts)
    this.transactions = transactions
    this.uncleHeaders = uncleHeaders
    this._common = this.header._common
    if (uncleHeaders.length > 0) {
      this.validateUncles()
      if (this._common.consensusType() === ConsensusType.ProofOfAuthority) {
        const msg = this._errorMsg(
          'Block initialization with uncleHeaders on a PoA network is not allowed'
        )
        throw new Error(msg)
      }
      if (this._common.consensusType() === ConsensusType.ProofOfStake) {
        const msg = this._errorMsg(
          'Block initialization with uncleHeaders on a PoS network is not allowed'
        )
        throw new Error(msg)
      }
    }

    const freeze = opts?.freeze ?? true
    if (freeze) {
      Object.freeze(this)
    }
  }

  /**
   * Returns a Buffer Array of the raw Buffers of this block, in order.
   */
  raw(): BlockBuffer {
    return [
      this.header.raw(),
      this.transactions.map((tx) =>
        tx.supports(Capability.EIP2718TypedTransaction) ? tx.serialize() : tx.raw()
      ) as Buffer[],
      this.uncleHeaders.map((uh) => uh.raw()),
    ]
  }

  /**
   * Returns the hash of the block.
   */
  hash(): Buffer {
    return this.header.hash()
  }

  /**
   * Determines if this block is the genesis block.
   */
  isGenesis(): boolean {
    return this.header.isGenesis()
  }

  /**
   * Returns the rlp encoding of the block.
   */
  serialize(): Buffer {
    return Buffer.from(RLP.encode(bufArrToArr(this.raw())))
  }

  /**
   * Generates transaction trie for validation.
   */
  async genTxTrie(): Promise<void> {
    const { transactions, txTrie } = this
    for (let i = 0; i < transactions.length; i++) {
      const tx = transactions[i]
      const key = Buffer.from(RLP.encode(i))
      const value = tx.serialize()
      await txTrie.put(key, value)
    }
  }

  /**
   * Validates the transaction trie by generating a trie
   * and do a check on the root hash.
   */
  async validateTransactionsTrie(): Promise<boolean> {
    let result
    if (this.transactions.length === 0) {
      result = this.header.transactionsTrie.equals(KECCAK256_RLP)
      return result
    }

    if (this.txTrie.root().equals(KECCAK256_RLP)) {
      await this.genTxTrie()
    }
    result = this.txTrie.root().equals(this.header.transactionsTrie)
    return result
  }

  /**
   * Validates transaction signatures and minimum gas requirements.
   *
   * @param stringError - If `true`, a string with the indices of the invalid txs is returned.
   */
  validateTransactions(): boolean
  validateTransactions(stringError: false): boolean
  validateTransactions(stringError: true): string[]
  validateTransactions(stringError = false) {
    const errors: string[] = []
    // eslint-disable-next-line prefer-const
    for (let [i, tx] of this.transactions.entries()) {
      const errs = <string[]>tx.validate(true)
      if (this._common.isActivatedEIP(1559) === true) {
        if (tx.supports(Capability.EIP1559FeeMarket)) {
          tx = tx as FeeMarketEIP1559Transaction
          if (tx.maxFeePerGas < this.header.baseFeePerGas!) {
            errs.push('tx unable to pay base fee (EIP-1559 tx)')
          }
        } else {
          tx = tx as Transaction
          if (tx.gasPrice < this.header.baseFeePerGas!) {
            errs.push('tx unable to pay base fee (non EIP-1559 tx)')
          }
        }
      }
      if (errs.length > 0) {
        errors.push(`errors at tx ${i}: ${errs.join(', ')}`)
      }
    }

    return stringError ? errors : errors.length === 0
  }

  /**
   * Validates the block data, throwing if invalid.
   * This can be checked on the Block itself without needing access to any parent block
   * It checks:
   * - All transactions are valid
   * - The transactions trie is valid
   * - The uncle hash is valid
   * @param onlyHeader if only passed the header, skip validating txTrie and unclesHash (default: false)
   */
  async validateData(onlyHeader: boolean = false): Promise<void> {
    const txErrors = this.validateTransactions(true)
    if (txErrors.length > 0) {
      const msg = this._errorMsg(`invalid transactions: ${txErrors.join(' ')}`)
      throw new Error(msg)
    }

    if (onlyHeader) {
      return
    }

    const validateTxTrie = await this.validateTransactionsTrie()
    if (!validateTxTrie) {
      const msg = this._errorMsg('invalid transaction trie')
      throw new Error(msg)
    }

    if (!this.validateUnclesHash()) {
      const msg = this._errorMsg('invalid uncle hash')
      throw new Error(msg)
    }
  }

  /**
   * Validates the uncle's hash.
   */
  validateUnclesHash(): boolean {
    const uncles = this.uncleHeaders.map((uh) => uh.raw())
    const raw = RLP.encode(bufArrToArr(uncles))
    return Buffer.from(keccak256(arrToBufArr(raw))).equals(this.header.uncleHash)
  }

  /**
   * Consistency checks for uncles included in the block, if any.
   *
   * Throws if invalid.
   *
   * The rules for uncles checked are the following:
   * Header has at most 2 uncles.
   * Header does not count an uncle twice.
   */
  validateUncles() {
    if (this.isGenesis()) {
      return
    }

    // Header has at most 2 uncles
    if (this.uncleHeaders.length > 2) {
      const msg = this._errorMsg('too many uncle headers')
      throw new Error(msg)
    }

    // Header does not count an uncle twice.
    const uncleHashes = this.uncleHeaders.map((header) => header.hash().toString('hex'))
    if (!(new Set(uncleHashes).size === uncleHashes.length)) {
      const msg = this._errorMsg('duplicate uncles')
      throw new Error(msg)
    }
  }

  /**
   * Returns the canonical difficulty for this block.
   *
   * @param parentBlock - the parent of this `Block`
   */
  ethashCanonicalDifficulty(parentBlock: Block): bigint {
    return this.header.ethashCanonicalDifficulty(parentBlock.header)
  }

  /**
   * Validates if the block gasLimit remains in the boundaries set by the protocol.
   * Throws if invalid
   *
   * @param parentBlock - the parent of this `Block`
   */
  validateGasLimit(parentBlock: Block) {
    return this.header.validateGasLimit(parentBlock.header)
  }

  /**
   * Returns the block in JSON format.
   */
  toJSON(): JsonBlock {
    return {
      header: this.header.toJSON(),
      transactions: this.transactions.map((tx) => tx.toJSON()),
      uncleHeaders: this.uncleHeaders.map((uh) => uh.toJSON()),
    }
  }

  /**
   * Return a compact error string representation of the object
   */
  public errorStr() {
    let hash = ''
    try {
      hash = bufferToHex(this.hash())
    } catch (e: any) {
      hash = 'error'
    }
    let hf = ''
    try {
      hf = this._common.hardfork()
    } catch (e: any) {
      hf = 'error'
    }
    let errorStr = `block number=${this.header.number} hash=${hash} `
    errorStr += `hf=${hf} baseFeePerGas=${this.header.baseFeePerGas ?? 'none'} `
    errorStr += `txs=${this.transactions.length} uncles=${this.uncleHeaders.length}`
    return errorStr
  }

  /**
   * Internal helper function to create an annotated error message
   *
   * @param msg Base error message
   * @hidden
   */
  protected _errorMsg(msg: string) {
    return `${msg} (${this.errorStr()})`
  }
}
