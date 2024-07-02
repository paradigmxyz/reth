import { Block, BlockHeader } from '@nomicfoundation/ethereumjs-block'
import {
  Chain,
  Common,
  ConsensusAlgorithm,
  ConsensusType,
  Hardfork,
} from '@nomicfoundation/ethereumjs-common'
import { KECCAK256_RLP, Lock } from '@nomicfoundation/ethereumjs-util'
import { MemoryLevel } from 'memory-level'

import { CasperConsensus, CliqueConsensus, EthashConsensus } from './consensus'
import { DBOp, DBSaveLookups, DBSetBlockOrHeader, DBSetHashToNumber, DBSetTD } from './db/helpers'
import { DBManager } from './db/manager'
import { DBTarget } from './db/operation'
import { genesisStateRoot } from './genesisStates'
import {} from './utils'

import type { Consensus } from './consensus'
import type { GenesisState } from './genesisStates'
import type { BlockchainInterface, BlockchainOptions, OnBlock } from './types'
import type { BlockData } from '@nomicfoundation/ethereumjs-block'
import type { CliqueConfig } from '@nomicfoundation/ethereumjs-common'
import type { BigIntLike } from '@nomicfoundation/ethereumjs-util'
import type { AbstractLevel } from 'abstract-level'

/**
 * This class stores and interacts with blocks.
 */
export class Blockchain implements BlockchainInterface {
  consensus: Consensus
  db: AbstractLevel<string | Buffer | Uint8Array, string | Buffer, string | Buffer>
  dbManager: DBManager

  private _genesisBlock?: Block /** The genesis block of this blockchain */
  private _customGenesisState?: GenesisState /** Custom genesis state */

  /**
   * The following two heads and the heads stored within the `_heads` always point
   * to a hash in the canonical chain and never to a stale hash.
   * With the exception of `_headHeaderHash` this does not necessarily need to be
   * the hash with the highest total difficulty.
   */
  /** The hash of the current head block */
  private _headBlockHash?: Buffer
  /** The hash of the current head header */
  private _headHeaderHash?: Buffer

  /**
   * A Map which stores the head of each key (for instance the "vm" key) which is
   * updated along a {@link Blockchain.iterator} method run and can be used to (re)run
   * non-verified blocks (for instance in the VM).
   */
  private _heads: { [key: string]: Buffer }

  protected _isInitialized = false
  private _lock: Lock

  _common: Common
  private _hardforkByHeadBlockNumber: boolean
  private readonly _validateConsensus: boolean
  private readonly _validateBlocks: boolean

  /**
   * Safe creation of a new Blockchain object awaiting the initialization function,
   * encouraged method to use when creating a blockchain object.
   *
   * @param opts Constructor options, see {@link BlockchainOptions}
   */

  public static async create(opts: BlockchainOptions = {}) {
    const blockchain = new Blockchain(opts)
    await blockchain._init(opts.genesisBlock)
    return blockchain
  }

  /**
   * Creates a blockchain from a list of block objects,
   * objects must be readable by {@link Block.fromBlockData}
   *
   * @param blockData List of block objects
   * @param opts Constructor options, see {@link BlockchainOptions}
   */
  public static async fromBlocksData(blocksData: BlockData[], opts: BlockchainOptions = {}) {
    const blockchain = await Blockchain.create(opts)
    for (const blockData of blocksData) {
      const block = Block.fromBlockData(blockData, {
        common: blockchain._common,
        hardforkByBlockNumber: true,
      })
      await blockchain.putBlock(block)
    }
    return blockchain
  }

  /**
   * Creates new Blockchain object.
   *
   * @deprecated The direct usage of this constructor is discouraged since
   * non-finalized async initialization might lead to side effects. Please
   * use the async {@link Blockchain.create} constructor instead (same API).
   *
   * @param opts An object with the options that this constructor takes. See
   * {@link BlockchainOptions}.
   */
  protected constructor(opts: BlockchainOptions = {}) {
    if (opts.common) {
      this._common = opts.common
    } else {
      const DEFAULT_CHAIN = Chain.Mainnet
      const DEFAULT_HARDFORK = Hardfork.Chainstart
      this._common = new Common({
        chain: DEFAULT_CHAIN,
        hardfork: DEFAULT_HARDFORK,
      })
    }

    this._hardforkByHeadBlockNumber = opts.hardforkByHeadBlockNumber ?? false
    this._validateConsensus = opts.validateConsensus ?? true
    this._validateBlocks = opts.validateBlocks ?? true
    this._customGenesisState = opts.genesisState

    this.db = opts.db ? opts.db : new MemoryLevel()
    this.dbManager = new DBManager(this.db, this._common)

    if (opts.consensus) {
      this.consensus = opts.consensus
    } else {
      switch (this._common.consensusAlgorithm()) {
        case ConsensusAlgorithm.Casper:
          this.consensus = new CasperConsensus()
          break
        case ConsensusAlgorithm.Clique:
          this.consensus = new CliqueConsensus()
          break
        case ConsensusAlgorithm.Ethash:
          this.consensus = new EthashConsensus()
          break
        default:
          throw new Error(`consensus algorithm ${this._common.consensusAlgorithm()} not supported`)
      }
    }

    if (this._validateConsensus) {
      if (this._common.consensusType() === ConsensusType.ProofOfWork) {
        if (this._common.consensusAlgorithm() !== ConsensusAlgorithm.Ethash) {
          throw new Error('consensus validation only supported for pow ethash algorithm')
        }
      }
      if (this._common.consensusType() === ConsensusType.ProofOfAuthority) {
        if (this._common.consensusAlgorithm() !== ConsensusAlgorithm.Clique) {
          throw new Error(
            'consensus (signature) validation only supported for poa clique algorithm'
          )
        }
      }
    }

    this._heads = {}

    this._lock = new Lock()

    if (opts.genesisBlock && !opts.genesisBlock.isGenesis()) {
      throw 'supplied block is not a genesis block'
    }
  }

  /**
   * Returns a deep copy of this {@link Blockchain} instance.
   *
   * Note: this does not make a copy of the underlying db
   * since it is unknown if the source is on disk or in memory.
   * This should not be a significant issue in most usage since
   * the queries will only reflect the instance's known data.
   * If you would like this copied blockchain to use another db
   * set the {@link db} of this returned instance to a copy of
   * the original.
   */
  copy(): Blockchain {
    const copiedBlockchain = Object.create(
      Object.getPrototypeOf(this),
      Object.getOwnPropertyDescriptors(this)
    )
    copiedBlockchain._common = this._common.copy()
    return copiedBlockchain
  }

  /**
   * This method is called in {@link Blockchain.create} and either sets up the DB or reads
   * values from the DB and makes these available to the consumers of
   * Blockchain.
   *
   * @hidden
   */
  private async _init(genesisBlock?: Block): Promise<void> {
    await this.consensus.setup({ blockchain: this })

    if (this._isInitialized) return
    let dbGenesisBlock
    try {
      const genesisHash = await this.dbManager.numberToHash(BigInt(0))
      dbGenesisBlock = await this.dbManager.getBlock(genesisHash)
    } catch (error: any) {
      if (error.code !== 'LEVEL_NOT_FOUND') {
        throw error
      }
    }

    if (!genesisBlock) {
      let stateRoot
      if (this._common.chainId() === BigInt(1) && this._customGenesisState === undefined) {
        // For mainnet use the known genesis stateRoot to quicken setup
        stateRoot = Buffer.from(
          'd7f8974fb5ac78d9ac099b9ad5018bedc2ce0a72dad1827a1709da30580f0544',
          'hex'
        )
      } else {
        stateRoot = await genesisStateRoot(this.genesisState())
      }
      genesisBlock = this.createGenesisBlock(stateRoot)
    }

    // If the DB has a genesis block, then verify that the genesis block in the
    // DB is indeed the Genesis block generated or assigned.
    if (dbGenesisBlock && !genesisBlock.hash().equals(dbGenesisBlock.hash())) {
      throw new Error(
        'The genesis block in the DB has a different hash than the provided genesis block.'
      )
    }

    const genesisHash = genesisBlock.hash()

    if (!dbGenesisBlock) {
      // If there is no genesis block put the genesis block in the DB.
      // For that TD, the BlockOrHeader, and the Lookups have to be saved.
      const dbOps: DBOp[] = []
      dbOps.push(DBSetTD(genesisBlock.header.difficulty, BigInt(0), genesisHash))
      DBSetBlockOrHeader(genesisBlock).map((op) => dbOps.push(op))
      DBSaveLookups(genesisHash, BigInt(0)).map((op) => dbOps.push(op))
      await this.dbManager.batch(dbOps)
      await this.consensus.genesisInit(genesisBlock)
    }

    // At this point, we can safely set the genesis:
    // it is either the one we put in the DB, or it is equal to the one
    // which we read from the DB.
    this._genesisBlock = genesisBlock

    // load verified iterator heads
    try {
      const heads = await this.dbManager.getHeads()
      this._heads = heads
    } catch (error: any) {
      if (error.code !== 'LEVEL_NOT_FOUND') {
        throw error
      }
      this._heads = {}
    }

    // load headerchain head
    try {
      const hash = await this.dbManager.getHeadHeader()
      this._headHeaderHash = hash
    } catch (error: any) {
      if (error.code !== 'LEVEL_NOT_FOUND') {
        throw error
      }
      this._headHeaderHash = genesisHash
    }

    // load blockchain head
    try {
      const hash = await this.dbManager.getHeadBlock()
      this._headBlockHash = hash
    } catch (error: any) {
      if (error.code !== 'LEVEL_NOT_FOUND') {
        throw error
      }
      this._headBlockHash = genesisHash
    }

    if (this._hardforkByHeadBlockNumber) {
      const latestHeader = await this._getHeader(this._headHeaderHash)
      const td = await this.getParentTD(latestHeader)
      await this.checkAndTransitionHardForkByNumber(latestHeader.number, td, latestHeader.timestamp)
    }

    this._isInitialized = true
  }

  /**
   * Run a function after acquiring a lock. It is implied that we have already
   * initialized the module (or we are calling this from the init function, like
   * `_setCanonicalGenesisBlock`)
   * @param action - function to run after acquiring a lock
   * @hidden
   */
  private async runWithLock<T>(action: () => Promise<T>): Promise<T> {
    try {
      await this._lock.acquire()
      const value = await action()
      return value
    } finally {
      this._lock.release()
    }
  }

  /**
   * Returns the specified iterator head.
   *
   * This function replaces the old {@link Blockchain.getHead} method. Note that
   * the function deviates from the old behavior and returns the
   * genesis hash instead of the current head block if an iterator
   * has not been run. This matches the behavior of {@link Blockchain.iterator}.
   *
   * @param name - Optional name of the iterator head (default: 'vm')
   */
  async getIteratorHead(name = 'vm'): Promise<Block> {
    return this.runWithLock<Block>(async () => {
      // if the head is not found return the genesis hash
      const hash = this._heads[name] ?? this.genesisBlock.hash()
      const block = await this.getBlock(hash)
      return block
    })
  }

  /**
   * Returns the specified iterator head.
   *
   * @param name - Optional name of the iterator head (default: 'vm')
   *
   * @deprecated use {@link Blockchain.getIteratorHead} instead.
   * Note that {@link Blockchain.getIteratorHead} doesn't return
   * the `headHeader` but the genesis hash as an initial iterator
   * head value (now matching the behavior of {@link Blockchain.iterator}
   * on a first run)
   */
  async getHead(name = 'vm'): Promise<Block> {
    return this.runWithLock<Block>(async () => {
      // if the head is not found return the headHeader
      const hash = this._heads[name] ?? this._headBlockHash
      if (hash === undefined) throw new Error('No head found.')
      const block = await this.getBlock(hash)
      return block
    })
  }

  /**
   * Returns the latest header in the canonical chain.
   */
  async getCanonicalHeadHeader(): Promise<BlockHeader> {
    return this.runWithLock<BlockHeader>(async () => {
      if (!this._headHeaderHash) throw new Error('No head header set')
      const block = await this.getBlock(this._headHeaderHash)
      return block.header
    })
  }

  /**
   * Returns the latest full block in the canonical chain.
   */
  async getCanonicalHeadBlock(): Promise<Block> {
    return this.runWithLock<Block>(async () => {
      if (!this._headBlockHash) throw new Error('No head block set')
      return this.getBlock(this._headBlockHash)
    })
  }

  /**
   * Adds blocks to the blockchain.
   *
   * If an invalid block is met the function will throw, blocks before will
   * nevertheless remain in the DB. If any of the saved blocks has a higher
   * total difficulty than the current max total difficulty the canonical
   * chain is rebuilt and any stale heads/hashes are overwritten.
   * @param blocks - The blocks to be added to the blockchain
   */
  async putBlocks(blocks: Block[]) {
    for (let i = 0; i < blocks.length; i++) {
      await this.putBlock(blocks[i])
    }
  }

  /**
   * Adds a block to the blockchain.
   *
   * If the block is valid and has a higher total difficulty than the current
   * max total difficulty, the canonical chain is rebuilt and any stale
   * heads/hashes are overwritten.
   * @param block - The block to be added to the blockchain
   */
  async putBlock(block: Block) {
    await this._putBlockOrHeader(block)
  }

  /**
   * Adds many headers to the blockchain.
   *
   * If an invalid header is met the function will throw, headers before will
   * nevertheless remain in the DB. If any of the saved headers has a higher
   * total difficulty than the current max total difficulty the canonical
   * chain is rebuilt and any stale heads/hashes are overwritten.
   * @param headers - The headers to be added to the blockchain
   */
  async putHeaders(headers: Array<any>) {
    for (let i = 0; i < headers.length; i++) {
      await this.putHeader(headers[i])
    }
  }

  /**
   * Adds a header to the blockchain.
   *
   * If this header is valid and it has a higher total difficulty than the current
   * max total difficulty, the canonical chain is rebuilt and any stale
   * heads/hashes are overwritten.
   * @param header - The header to be added to the blockchain
   */
  async putHeader(header: BlockHeader) {
    await this._putBlockOrHeader(header)
  }

  /**
   * Resets the canonical chain to canonicalHead number
   *
   * This updates the head hashes (if affected) to the hash corresponding to
   * canonicalHead and cleans up canonical references greater than canonicalHead
   * @param canonicalHead - The number to which chain should be reset to
   */

  async resetCanonicalHead(canonicalHead: bigint) {
    await this.runWithLock<void>(async () => {
      const hash = await this.dbManager.numberToHash(canonicalHead)
      const header = await this._getHeader(hash, canonicalHead)
      const td = await this.getParentTD(header)

      const dbOps: DBOp[] = []
      await this._deleteCanonicalChainReferences(canonicalHead + BigInt(1), hash, dbOps)
      const ops = dbOps.concat(this._saveHeadOps())

      await this.dbManager.batch(ops)
      await this.checkAndTransitionHardForkByNumber(canonicalHead, td, header.timestamp)
    })
  }

  /**
   * Entrypoint for putting any block or block header. Verifies this block,
   * checks the total TD: if this TD is higher than the current highest TD, we
   * have thus found a new canonical block and have to rewrite the canonical
   * chain. This also updates the head block hashes. If any of the older known
   * canonical chains just became stale, then we also reset every _heads header
   * which points to a stale header to the last verified header which was in the
   * old canonical chain, but also in the new canonical chain. This thus rolls
   * back these headers so that these can be updated to the "new" canonical
   * header using the iterator method.
   * @hidden
   */
  private async _putBlockOrHeader(item: Block | BlockHeader) {
    await this.runWithLock<void>(async () => {
      // Save the current sane state incase _putBlockOrHeader midway with some
      // dirty changes in head trackers
      const oldHeads = Object.assign({}, this._heads)
      const oldHeadHeaderHash = this._headHeaderHash
      const oldHeadBlockHash = this._headBlockHash
      try {
        const block =
          item instanceof BlockHeader
            ? new Block(item, undefined, undefined, {
                common: item._common,
              })
            : item
        const isGenesis = block.isGenesis()

        // we cannot overwrite the Genesis block after initializing the Blockchain
        if (isGenesis) {
          throw new Error('Cannot put a genesis block: create a new Blockchain')
        }

        const { header } = block
        const blockHash = header.hash()
        const blockNumber = header.number
        let td = header.difficulty
        const currentTd = { header: BigInt(0), block: BigInt(0) }
        let dbOps: DBOp[] = []

        if (block._common.chainId() !== this._common.chainId()) {
          throw new Error('Chain mismatch while trying to put block or header')
        }

        if (this._validateBlocks && !isGenesis) {
          // this calls into `getBlock`, which is why we cannot lock yet
          await this.validateBlock(block)
        }

        if (this._validateConsensus) {
          await this.consensus.validateConsensus(block)
        }

        // set total difficulty in the current context scope
        if (this._headHeaderHash) {
          currentTd.header = await this.getTotalDifficulty(this._headHeaderHash)
        }
        if (this._headBlockHash) {
          currentTd.block = await this.getTotalDifficulty(this._headBlockHash)
        }

        // calculate the total difficulty of the new block
        const parentTd = await this.getParentTD(header)
        if (!block.isGenesis()) {
          td += parentTd
        }

        // save total difficulty to the database
        dbOps = dbOps.concat(DBSetTD(td, blockNumber, blockHash))

        // save header/block to the database
        dbOps = dbOps.concat(DBSetBlockOrHeader(block))

        let commonAncestor: undefined | BlockHeader
        let ancestorHeaders: undefined | BlockHeader[]
        // if total difficulty is higher than current, add it to canonical chain
        if (
          block.isGenesis() ||
          td > currentTd.header ||
          block._common.consensusType() === ConsensusType.ProofOfStake
        ) {
          const foundCommon = await this.findCommonAncestor(header)
          commonAncestor = foundCommon.commonAncestor
          ancestorHeaders = foundCommon.ancestorHeaders

          this._headHeaderHash = blockHash
          if (item instanceof Block) {
            this._headBlockHash = blockHash
          }
          if (this._hardforkByHeadBlockNumber) {
            await this.checkAndTransitionHardForkByNumber(blockNumber, parentTd, header.timestamp)
          }

          // delete higher number assignments and overwrite stale canonical chain
          await this._deleteCanonicalChainReferences(blockNumber + BigInt(1), blockHash, dbOps)
          // from the current header block, check the blockchain in reverse (i.e.
          // traverse `parentHash`) until `numberToHash` matches the current
          // number/hash in the canonical chain also: overwrite any heads if these
          // heads are stale in `_heads` and `_headBlockHash`
          await this._rebuildCanonical(header, dbOps)
        } else {
          // the TD is lower than the current highest TD so we will add the block
          // to the DB, but will not mark it as the canonical chain.
          if (td > currentTd.block && item instanceof Block) {
            this._headBlockHash = blockHash
          }
          // save hash to number lookup info even if rebuild not needed
          dbOps.push(DBSetHashToNumber(blockHash, blockNumber))
        }

        const ops = dbOps.concat(this._saveHeadOps())
        await this.dbManager.batch(ops)

        await this.consensus.newBlock(block, commonAncestor, ancestorHeaders)
      } catch (e) {
        // restore head to the previouly sane state
        this._heads = oldHeads
        this._headHeaderHash = oldHeadHeaderHash
        this._headBlockHash = oldHeadBlockHash
        throw e
      }
    })
  }

  /**
   * Validates a block header, throwing if invalid. It is being validated against the reported `parentHash`.
   * It verifies the current block against the `parentHash`:
   * - The `parentHash` is part of the blockchain (it is a valid header)
   * - Current block number is parent block number + 1
   * - Current block has a strictly higher timestamp
   * - Additional PoW checks ->
   *   - Current block has valid difficulty and gas limit
   *   - In case that the header is an uncle header, it should not be too old or young in the chain.
   * - Additional PoA clique checks ->
   *   - Checks on coinbase and mixHash
   *   - Current block has a timestamp diff greater or equal to PERIOD
   *   - Current block has difficulty correctly marked as INTURN or NOTURN
   * @param header - header to be validated
   * @param height - If this is an uncle header, this is the height of the block that is including it
   */
  public async validateHeader(header: BlockHeader, height?: bigint) {
    if (header.isGenesis()) {
      return
    }
    const parentHeader = (await this.getBlock(header.parentHash)).header

    const { number } = header
    if (number !== parentHeader.number + BigInt(1)) {
      throw new Error(`invalid number ${header.errorStr()}`)
    }

    if (header.timestamp <= parentHeader.timestamp) {
      throw new Error(`invalid timestamp ${header.errorStr()}`)
    }

    if (!(header._common.consensusType() === 'pos')) await this.consensus.validateDifficulty(header)

    if (this._common.consensusAlgorithm() === ConsensusAlgorithm.Clique) {
      const period = (this._common.consensusConfig() as CliqueConfig).period
      // Timestamp diff between blocks is lower than PERIOD (clique)
      if (parentHeader.timestamp + BigInt(period) > header.timestamp) {
        throw new Error(`invalid timestamp diff (lower than period) ${header.errorStr()}`)
      }
    }

    header.validateGasLimit(parentHeader)

    if (height !== undefined) {
      const dif = height - parentHeader.number

      if (!(dif < BigInt(8) && dif > BigInt(1))) {
        throw new Error(
          `uncle block has a parent that is too old or too young ${header.errorStr()}`
        )
      }
    }

    // check blockchain dependent EIP1559 values
    if (header._common.isActivatedEIP(1559) === true) {
      // check if the base fee is correct
      let expectedBaseFee
      const londonHfBlock = this._common.hardforkBlock(Hardfork.London)
      const isInitialEIP1559Block = number === londonHfBlock
      if (isInitialEIP1559Block) {
        expectedBaseFee = header._common.param('gasConfig', 'initialBaseFee')
      } else {
        expectedBaseFee = parentHeader.calcNextBaseFee()
      }

      if (header.baseFeePerGas! !== expectedBaseFee) {
        throw new Error(`Invalid block: base fee not correct ${header.errorStr()}`)
      }
    }
  }

  /**
   * Validates a block, by validating the header against the current chain, any uncle headers, and then
   * whether the block is internally consistent
   * @param block block to be validated
   */
  public async validateBlock(block: Block) {
    await this.validateHeader(block.header)
    await this._validateUncleHeaders(block)
    await block.validateData(false)
    // TODO: Rethink how validateHeader vs validateBlobTransactions works since the parentHeader is retrieved multiple times
    // (one for each uncle header and then for validateBlobTxs).
    const parentBlock = await this.getBlock(block.header.parentHash)
    block.validateBlobTransactions(parentBlock.header)
  }
  /**
   * The following rules are checked in this method:
   * Uncle Header is a valid header.
   * Uncle Header is an orphan, i.e. it is not one of the headers of the canonical chain.
   * Uncle Header has a parentHash which points to the canonical chain. This parentHash is within the last 7 blocks.
   * Uncle Header is not already included as uncle in another block.
   * @param block - block for which uncles are being validated
   */
  private async _validateUncleHeaders(block: Block) {
    const uncleHeaders = block.uncleHeaders
    if (uncleHeaders.length === 0) {
      return
    }

    // Each Uncle Header is a valid header
    await Promise.all(uncleHeaders.map((uh) => this.validateHeader(uh, block.header.number)))

    // Check how many blocks we should get in order to validate the uncle.
    // In the worst case, we get 8 blocks, in the best case, we only get 1 block.
    const canonicalBlockMap: Block[] = []
    let lowestUncleNumber = block.header.number

    uncleHeaders.map((header) => {
      if (header.number < lowestUncleNumber) {
        lowestUncleNumber = header.number
      }
    })

    // Helper variable: set hash to `true` if hash is part of the canonical chain
    const canonicalChainHashes: { [key: string]: boolean } = {}

    // Helper variable: set hash to `true` if uncle hash is included in any canonical block
    const includedUncles: { [key: string]: boolean } = {}

    // Due to the header validation check above, we know that `getBlocks` is between 1 and 8 inclusive.
    const getBlocks = Number(block.header.number - lowestUncleNumber + BigInt(1))

    // See Geth: https://github.com/ethereum/go-ethereum/blob/b63bffe8202d46ea10ac8c4f441c582642193ac8/consensus/ethash/consensus.go#L207
    // Here we get the necessary blocks from the chain.
    let parentHash = block.header.parentHash
    for (let i = 0; i < getBlocks; i++) {
      const parentBlock = await this.getBlock(parentHash)
      canonicalBlockMap.push(parentBlock)

      // mark block hash as part of the canonical chain
      canonicalChainHashes[parentBlock.hash().toString('hex')] = true

      // for each of the uncles, mark the uncle as included
      parentBlock.uncleHeaders.map((uh) => {
        includedUncles[uh.hash().toString('hex')] = true
      })

      parentHash = parentBlock.header.parentHash
    }

    // Here we check:
    // Uncle Header is an orphan, i.e. it is not one of the headers of the canonical chain.
    // Uncle Header is not already included as uncle in another block.
    // Uncle Header has a parentHash which points to the canonical chain.

    uncleHeaders.map((uh) => {
      const uncleHash = uh.hash().toString('hex')
      const parentHash = uh.parentHash.toString('hex')

      if (!canonicalChainHashes[parentHash]) {
        throw new Error(
          `The parent hash of the uncle header is not part of the canonical chain ${block.errorStr()}`
        )
      }

      if (includedUncles[uncleHash]) {
        throw new Error(`The uncle is already included in the canonical chain ${block.errorStr()}`)
      }

      if (canonicalChainHashes[uncleHash]) {
        throw new Error(`The uncle is a canonical block ${block.errorStr()}`)
      }
    })
  }

  /**
   * Gets a block by its hash or number.  If a number is provided, the returned
   * block will be the canonical block at that number in the chain
   *
   * @param blockId - The block's hash or number. If a hash is provided, then
   * this will be immediately looked up, otherwise it will wait until we have
   * unlocked the DB
   */
  async getBlock(blockId: Buffer | number | bigint): Promise<Block> {
    // cannot wait for a lock here: it is used both in `validate` of `Block`
    // (calls `getBlock` to get `parentHash`) it is also called from `runBlock`
    // in the `VM` if we encounter a `BLOCKHASH` opcode: then a bigint is used we
    // need to then read the block from the canonical chain Q: is this safe? We
    // know it is OK if we call it from the iterator... (runBlock)
    try {
      return await this.dbManager.getBlock(blockId)
    } catch (error: any) {
      if (error.code === 'LEVEL_NOT_FOUND') {
        if (typeof blockId === 'object') {
          error.message = `Block with hash ${blockId.toString('hex')} not found in DB (NotFound)`
        } else {
          error.message = `Block number ${blockId} not found in DB (NotFound)`
        }
      }
      throw error
    }
  }

  /**
   * Gets total difficulty for a block specified by hash and number
   */
  public async getTotalDifficulty(hash: Buffer, number?: bigint): Promise<bigint> {
    if (number === undefined) {
      number = await this.dbManager.hashToNumber(hash)
    }
    return this.dbManager.getTotalDifficulty(hash, number)
  }

  /**
   * Gets total difficulty for a header's parent, helpful for determining terminal block
   * @param header - Block header whose parent td is desired
   */
  public async getParentTD(header: BlockHeader): Promise<bigint> {
    return header.number === BigInt(0)
      ? header.difficulty
      : this.getTotalDifficulty(header.parentHash, header.number - BigInt(1))
  }

  /**
   * Looks up many blocks relative to blockId Note: due to `GetBlockHeaders
   * (0x03)` (ETH wire protocol) we have to support skip/reverse as well.
   * @param blockId - The block's hash or number
   * @param maxBlocks - Max number of blocks to return
   * @param skip - Number of blocks to skip apart
   * @param reverse - Fetch blocks in reverse
   */
  async getBlocks(
    blockId: Buffer | bigint | number,
    maxBlocks: number,
    skip: number,
    reverse: boolean
  ): Promise<Block[]> {
    return this.runWithLock<Block[]>(async () => {
      const blocks: Block[] = []
      let i = -1

      const nextBlock = async (blockId: Buffer | bigint | number): Promise<any> => {
        let block
        try {
          block = await this.getBlock(blockId)
        } catch (error: any) {
          if (error.code !== 'LEVEL_NOT_FOUND') {
            throw error
          }
          return
        }
        i++
        const nextBlockNumber = block.header.number + BigInt(reverse ? -1 : 1)
        if (i !== 0 && skip && i % (skip + 1) !== 0) {
          return nextBlock(nextBlockNumber)
        }
        blocks.push(block)
        if (blocks.length < maxBlocks) {
          await nextBlock(nextBlockNumber)
        }
      }

      await nextBlock(blockId)
      return blocks
    })
  }

  /**
   * Given an ordered array, returns an array of hashes that are not in the
   * blockchain yet. Uses binary search to find out what hashes are missing.
   * Therefore, the array needs to be ordered upon number.
   * @param hashes - Ordered array of hashes (ordered on `number`).
   */
  async selectNeededHashes(hashes: Array<Buffer>): Promise<Buffer[]> {
    return this.runWithLock<Buffer[]>(async () => {
      let max: number
      let mid: number
      let min: number

      max = hashes.length - 1
      mid = min = 0

      while (max >= min) {
        let number
        try {
          number = await this.dbManager.hashToNumber(hashes[mid])
        } catch (error: any) {
          if (error.code !== 'LEVEL_NOT_FOUND') {
            throw error
          }
        }
        if (number !== undefined) {
          min = mid + 1
        } else {
          max = mid - 1
        }
        mid = Math.floor((min + max) / 2)
      }
      return hashes.slice(min)
    })
  }

  /**
   * Completely deletes a block from the blockchain including any references to
   * this block. If this block was in the canonical chain, then also each child
   * block of this block is deleted Also, if this was a canonical block, each
   * head header which is part of this now stale chain will be set to the
   * parentHeader of this block An example reason to execute is when running the
   * block in the VM invalidates this block: this will then reset the canonical
   * head to the past block (which has been validated in the past by the VM, so
   * we can be sure it is correct).
   * @param blockHash - The hash of the block to be deleted
   */
  async delBlock(blockHash: Buffer) {
    // Q: is it safe to make this not wait for a lock? this is called from
    // `BlockchainTestsRunner` in case `runBlock` throws (i.e. the block is invalid).
    // But is this the way to go? If we know this is called from the
    // iterator we are safe, but if this is called from anywhere
    // else then this might lead to a concurrency problem?
    await this._delBlock(blockHash)
  }

  /**
   * @hidden
   */
  private async _delBlock(blockHash: Buffer) {
    const dbOps: DBOp[] = []

    // get header
    const header = await this._getHeader(blockHash)
    const blockHeader = header
    const blockNumber = blockHeader.number
    const parentHash = blockHeader.parentHash

    // check if block is in the canonical chain
    const canonicalHash = await this.safeNumberToHash(blockNumber)

    const inCanonical = canonicalHash !== false && canonicalHash.equals(blockHash)

    // delete the block, and if block is in the canonical chain, delete all
    // children as well
    await this._delChild(blockHash, blockNumber, inCanonical ? parentHash : null, dbOps)

    // delete all number to hash mappings for deleted block number and above
    if (inCanonical) {
      await this._deleteCanonicalChainReferences(blockNumber, parentHash, dbOps)
    }

    await this.dbManager.batch(dbOps)
  }

  /**
   * Updates the `DatabaseOperation` list to delete a block from the DB,
   * identified by `blockHash` and `blockNumber`. Deletes fields from `Header`,
   * `Body`, `HashToNumber` and `TotalDifficulty` tables. If child blocks of
   * this current block are in the canonical chain, delete these as well. Does
   * not actually commit these changes to the DB. Sets `_headHeaderHash` and
   * `_headBlockHash` to `headHash` if any of these matches the current child to
   * be deleted.
   * @param blockHash - the block hash to delete
   * @param blockNumber - the number corresponding to the block hash
   * @param headHash - the current head of the chain (if null, do not update
   * `_headHeaderHash` and `_headBlockHash`)
   * @param ops - the `DatabaseOperation` list to add the delete operations to
   * @hidden
   */
  private async _delChild(
    blockHash: Buffer,
    blockNumber: bigint,
    headHash: Buffer | null,
    ops: DBOp[]
  ) {
    // delete header, body, hash to number mapping and td
    ops.push(DBOp.del(DBTarget.Header, { blockHash, blockNumber }))
    ops.push(DBOp.del(DBTarget.Body, { blockHash, blockNumber }))
    ops.push(DBOp.del(DBTarget.HashToNumber, { blockHash }))
    ops.push(DBOp.del(DBTarget.TotalDifficulty, { blockHash, blockNumber }))

    if (!headHash) {
      return
    }

    if (this._headHeaderHash?.equals(blockHash) === true) {
      this._headHeaderHash = headHash
    }

    if (this._headBlockHash?.equals(blockHash) === true) {
      this._headBlockHash = headHash
    }

    try {
      const childHeader = await this.getCanonicalHeader(blockNumber + BigInt(1))
      await this._delChild(childHeader.hash(), childHeader.number, headHash, ops)
    } catch (error: any) {
      if (error.code !== 'LEVEL_NOT_FOUND') {
        throw error
      }
    }
  }

  /**
   * Iterates through blocks starting at the specified iterator head and calls
   * the onBlock function on each block. The current location of an iterator
   * head can be retrieved using {@link Blockchain.getIteratorHead}.
   *
   * @param name - Name of the state root head
   * @param onBlock - Function called on each block with params (block, reorg)
   * @param maxBlocks - How many blocks to run. By default, run all unprocessed blocks in the canonical chain.
   * @param releaseLockOnCallback - Do not lock the blockchain for running the callback (default: `false`)
   * @returns number of blocks actually iterated
   */
  async iterator(
    name: string,
    onBlock: OnBlock,
    maxBlocks?: number,
    releaseLockOnCallback?: boolean
  ): Promise<number> {
    return this.runWithLock<number>(async (): Promise<number> => {
      let headHash = this._heads[name] ?? this.genesisBlock.hash()

      if (typeof maxBlocks === 'number' && maxBlocks < 0) {
        throw 'If maxBlocks is provided, it has to be a non-negative number'
      }

      let headBlockNumber = await this.dbManager.hashToNumber(headHash)
      let nextBlockNumber = headBlockNumber + BigInt(1)
      let blocksRanCounter = 0
      let lastBlock: Block | undefined

      while (maxBlocks !== blocksRanCounter) {
        try {
          let nextBlock = await this.getBlock(nextBlockNumber)
          const reorg = lastBlock ? !lastBlock.hash().equals(nextBlock.header.parentHash) : false
          if (reorg) {
            // If reorg has happened, the _heads must have been updated so lets reload the counters
            headHash = this._heads[name] ?? this.genesisBlock.hash()
            headBlockNumber = await this.dbManager.hashToNumber(headHash)
            nextBlockNumber = headBlockNumber + BigInt(1)
            nextBlock = await this.getBlock(nextBlockNumber)
          }
          this._heads[name] = nextBlock.hash()
          lastBlock = nextBlock
          if (releaseLockOnCallback === true) {
            this._lock.release()
          }
          try {
            await onBlock(nextBlock, reorg)
          } finally {
            if (releaseLockOnCallback === true) {
              await this._lock.acquire()
            }
          }
          nextBlockNumber++
          blocksRanCounter++
        } catch (error: any) {
          if (error.code === 'LEVEL_NOT_FOUND') {
            break
          } else {
            throw error
          }
        }
      }

      await this._saveHeads()
      return blocksRanCounter
    })
  }

  /**
   * Set header hash of a certain `tag`.
   * When calling the iterator, the iterator will start running the first child block after the header hash currently stored.
   * @param tag - The tag to save the headHash to
   * @param headHash - The head hash to save
   */
  async setIteratorHead(tag: string, headHash: Buffer) {
    await this.runWithLock<void>(async () => {
      this._heads[tag] = headHash
      await this._saveHeads()
    })
  }

  /* Methods regarding reorg operations */

  /**
   * Find the common ancestor of the new block and the old block.
   * @param newHeader - the new block header
   */
  private async findCommonAncestor(newHeader: BlockHeader) {
    if (!this._headHeaderHash) throw new Error('No head header set')
    const ancestorHeaders = new Set<BlockHeader>()

    let { header } = await this.getBlock(this._headHeaderHash)
    if (header.number > newHeader.number) {
      header = await this.getCanonicalHeader(newHeader.number)
      ancestorHeaders.add(header)
    } else {
      while (header.number !== newHeader.number && newHeader.number > BigInt(0)) {
        newHeader = await this._getHeader(newHeader.parentHash, newHeader.number - BigInt(1))
        ancestorHeaders.add(newHeader)
      }
    }
    if (header.number !== newHeader.number) {
      throw new Error('Failed to find ancient header')
    }
    while (!header.hash().equals(newHeader.hash()) && header.number > BigInt(0)) {
      header = await this.getCanonicalHeader(header.number - BigInt(1))
      ancestorHeaders.add(header)
      newHeader = await this._getHeader(newHeader.parentHash, newHeader.number - BigInt(1))
      ancestorHeaders.add(newHeader)
    }
    if (!header.hash().equals(newHeader.hash())) {
      throw new Error('Failed to find ancient header')
    }
    return {
      commonAncestor: header,
      ancestorHeaders: Array.from(ancestorHeaders),
    }
  }

  /**
   * Pushes DB operations to delete canonical number assignments for specified
   * block number and above. This only deletes `NumberToHash` references and not
   * the blocks themselves. Note: this does not write to the DB but only pushes
   * to a DB operations list.
   * @param blockNumber - the block number from which we start deleting
   * canonical chain assignments (including this block)
   * @param headHash - the hash of the current canonical chain head. The _heads
   * reference matching any hash of any of the deleted blocks will be set to
   * this
   * @param ops - the DatabaseOperation list to write DatabaseOperations to
   * @hidden
   */
  private async _deleteCanonicalChainReferences(
    blockNumber: bigint,
    headHash: Buffer,
    ops: DBOp[]
  ) {
    let hash: Buffer | false

    hash = await this.safeNumberToHash(blockNumber)
    while (hash !== false) {
      ops.push(DBOp.del(DBTarget.NumberToHash, { blockNumber }))

      // reset stale iterator heads to current canonical head this can, for
      // instance, make the VM run "older" (i.e. lower number blocks than last
      // executed block) blocks to verify the chain up to the current, actual,
      // head.
      for (const name of Object.keys(this._heads)) {
        if (this._heads[name].equals(hash)) {
          this._heads[name] = headHash
        }
      }

      // reset stale headBlock to current canonical
      if (this._headBlockHash?.equals(hash) === true) {
        this._headBlockHash = headHash
      }
      // reset stale headBlock to current canonical
      if (this._headHeaderHash?.equals(hash) === true) {
        this._headHeaderHash = headHash
      }

      blockNumber++

      hash = await this.safeNumberToHash(blockNumber)
    }
  }

  /**
   * Given a `header`, put all operations to change the canonical chain directly
   * into `ops`. This walks the supplied `header` backwards. It is thus assumed
   * that this header should be canonical header. For each header the
   * corresponding hash corresponding to the current canonical chain in the DB
   * is checked. If the number => hash reference does not correspond to the
   * reference in the DB, we overwrite this reference with the implied number =>
   * hash reference Also, each `_heads` member is checked; if these point to a
   * stale hash, then the hash which we terminate the loop (i.e. the first hash
   * which matches the number => hash of the implied chain) is put as this stale
   * head hash. The same happens to _headBlockHash.
   * @param header - The canonical header.
   * @param ops - The database operations list.
   * @hidden
   */
  private async _rebuildCanonical(header: BlockHeader, ops: DBOp[]) {
    let currentNumber = header.number
    let currentCanonicalHash: Buffer = header.hash()

    // track the staleHash: this is the hash currently in the DB which matches
    // the block number of the provided header.
    let staleHash: Buffer | false = false
    let staleHeads: string[] = []
    let staleHeadBlock = false

    const loopCondition = async () => {
      staleHash = await this.safeNumberToHash(currentNumber)
      currentCanonicalHash = header.hash()
      return staleHash === false || !currentCanonicalHash.equals(staleHash)
    }

    while (await loopCondition()) {
      // handle genesis block
      const blockHash = header.hash()
      const blockNumber = header.number

      if (blockNumber === BigInt(0)) {
        break
      }

      DBSaveLookups(blockHash, blockNumber).map((op) => {
        ops.push(op)
      })

      // mark each key `_heads` which is currently set to the hash in the DB as
      // stale to overwrite later in `_deleteCanonicalChainReferences`.
      for (const name of Object.keys(this._heads)) {
        if (staleHash && this._heads[name].equals(staleHash)) {
          staleHeads.push(name)
        }
      }
      // flag stale headBlock for reset
      if (staleHash && this._headBlockHash?.equals(staleHash) === true) {
        staleHeadBlock = true
      }

      try {
        header = await this._getHeader(header.parentHash, --currentNumber)
      } catch (error: any) {
        staleHeads = []
        if (error.code !== 'LEVEL_NOT_FOUND') {
          throw error
        }
        break
      }
    }
    // When the stale hash is equal to the blockHash of the provided header,
    // set stale heads to last previously valid canonical block
    for (const name of staleHeads) {
      this._heads[name] = currentCanonicalHash
    }
    // set stale headBlock to last previously valid canonical block
    if (staleHeadBlock) {
      this._headBlockHash = currentCanonicalHash
    }
  }

  /* Helper functions */

  /**
   * Builds the `DatabaseOperation[]` list which describes the DB operations to
   * write the heads, head header hash and the head header block to the DB
   * @hidden
   */
  private _saveHeadOps(): DBOp[] {
    return [
      DBOp.set(DBTarget.Heads, this._heads),
      DBOp.set(DBTarget.HeadHeader, this._headHeaderHash!),
      DBOp.set(DBTarget.HeadBlock, this._headBlockHash!),
    ]
  }

  /**
   * Gets the `DatabaseOperation[]` list to save `_heads`, `_headHeaderHash` and
   * `_headBlockHash` and writes these to the DB
   * @hidden
   */
  private async _saveHeads() {
    return this.dbManager.batch(this._saveHeadOps())
  }

  /**
   * Gets a header by hash and number. Header can exist outside the canonical
   * chain
   *
   * @hidden
   */
  private async _getHeader(hash: Buffer, number?: bigint) {
    if (number === undefined) {
      number = await this.dbManager.hashToNumber(hash)
    }
    return this.dbManager.getHeader(hash, number)
  }

  async checkAndTransitionHardForkByNumber(
    number: bigint,
    td?: BigIntLike,
    timestamp?: BigIntLike
  ): Promise<void> {
    this._common.setHardforkByBlockNumber(number, td, timestamp)

    // If custom consensus algorithm is used, skip merge hardfork consensus checks
    if (!Object.values(ConsensusAlgorithm).includes(this.consensus.algorithm as ConsensusAlgorithm))
      return

    switch (this._common.consensusAlgorithm()) {
      case ConsensusAlgorithm.Casper:
        if (!(this.consensus instanceof CasperConsensus)) {
          this.consensus = new CasperConsensus()
        }
        break
      case ConsensusAlgorithm.Clique:
        if (!(this.consensus instanceof CliqueConsensus)) {
          this.consensus = new CliqueConsensus()
        }
        break
      case ConsensusAlgorithm.Ethash:
        if (!(this.consensus instanceof EthashConsensus)) {
          this.consensus = new EthashConsensus()
        }
        break
      default:
        throw new Error(`consensus algorithm ${this._common.consensusAlgorithm()} not supported`)
    }
    await this.consensus.setup({ blockchain: this })
  }

  /**
   * Gets a header by number. Header must be in the canonical chain
   */
  async getCanonicalHeader(number: bigint) {
    const hash = await this.dbManager.numberToHash(number)
    return this._getHeader(hash, number)
  }

  /**
   * This method either returns a Buffer if there exists one in the DB or if it
   * does not exist (DB throws a `NotFoundError`) then return false If DB throws
   * any other error, this function throws.
   * @param number
   */
  async safeNumberToHash(number: bigint): Promise<Buffer | false> {
    try {
      const hash = await this.dbManager.numberToHash(number)
      return hash
    } catch (error: any) {
      if (error.code !== 'LEVEL_NOT_FOUND') {
        throw error
      }
      return false
    }
  }

  /**
   * The genesis {@link Block} for the blockchain.
   */
  get genesisBlock(): Block {
    if (!this._genesisBlock) throw new Error('genesis block not set (init may not be finished)')
    return this._genesisBlock
  }

  /**
   * Creates a genesis {@link Block} for the blockchain with params from {@link Common.genesis}
   * @param stateRoot The genesis stateRoot
   */
  createGenesisBlock(stateRoot: Buffer): Block {
    const common = this._common.copy()
    common.setHardforkByBlockNumber(
      0,
      BigInt(common.genesis().difficulty),
      common.genesis().timestamp
    )

    const header: BlockData['header'] = {
      ...common.genesis(),
      number: 0,
      stateRoot,
      withdrawalsRoot: common.isActivatedEIP(4895) ? KECCAK256_RLP : undefined,
      excessDataGas: common.isActivatedEIP(4844) ? BigInt(0) : undefined,
    }
    if (common.consensusType() === 'poa') {
      if (common.genesis().extraData) {
        // Ensure exta data is populated from genesis data if provided
        header.extraData = common.genesis().extraData
      } else {
        // Add required extraData (32 bytes vanity + 65 bytes filled with zeroes
        header.extraData = Buffer.concat([Buffer.alloc(32), Buffer.alloc(65).fill(0)])
      }
    }
    return Block.fromBlockData(
      { header, withdrawals: common.isActivatedEIP(4895) ? [] : undefined },
      { common }
    )
  }

  /**
   * Returns the genesis state of the blockchain.
   * All values are provided as hex-prefixed strings.
   */
  genesisState(): GenesisState {
    if (this._customGenesisState) {
      return this._customGenesisState
    }
    // Use require statements here in favor of import statements
    // to load json files on demand
    // (high memory usage by large mainnet.json genesis state file)
    switch (this._common.chainName()) {
      case 'mainnet':
        return require('./genesisStates/mainnet.json')
      case 'ropsten':
        return require('./genesisStates/ropsten.json')
      case 'rinkeby':
        return require('./genesisStates/rinkeby.json')
      case 'goerli':
        return require('./genesisStates/goerli.json')
      case 'sepolia':
        return require('./genesisStates/sepolia.json')
    }

    return {}
  }
}
