import { Blockchain } from '@nomicfoundation/ethereumjs-blockchain'
import { Chain, Common } from '@nomicfoundation/ethereumjs-common'
import { EVM, getActivePrecompiles } from '@nomicfoundation/ethereumjs-evm'
import { DefaultStateManager } from '@nomicfoundation/ethereumjs-statemanager'
import { Account, Address, TypeOutput, toType } from '@nomicfoundation/ethereumjs-util'
import AsyncEventEmitter = require('async-eventemitter')
import { promisify } from 'util'

import { buildBlock } from './buildBlock'
import { EEI } from './eei/eei'
import { runBlock } from './runBlock'
import { runTx } from './runTx'

import type { BlockBuilder } from './buildBlock'
import type {
  BuildBlockOpts,
  RunBlockOpts,
  RunBlockResult,
  RunTxOpts,
  RunTxResult,
  VMEvents,
  VMOpts,
} from './types'
import type { BlockchainInterface } from '@nomicfoundation/ethereumjs-blockchain'
import type { EEIInterface, EVMInterface } from '@nomicfoundation/ethereumjs-evm'
import type { StateManager } from '@nomicfoundation/ethereumjs-statemanager'

/**
 * Execution engine which can be used to run a blockchain, individual
 * blocks, individual transactions, or snippets of EVM bytecode.
 *
 * This class is an AsyncEventEmitter, please consult the README to learn how to use it.
 */
export class VM {
  /**
   * The StateManager used by the VM
   */
  readonly stateManager: StateManager

  /**
   * The blockchain the VM operates on
   */
  readonly blockchain: BlockchainInterface

  readonly _common: Common

  readonly events: AsyncEventEmitter<VMEvents>

  /**
   * The EVM used for bytecode execution
   */
  readonly evm: EVMInterface
  readonly eei: EEIInterface

  protected readonly _opts: VMOpts
  protected _isInitialized: boolean = false

  protected readonly _hardforkByBlockNumber: boolean
  protected readonly _hardforkByTTD?: bigint

  /**
   * Cached emit() function, not for public usage
   * set to public due to implementation internals
   * @hidden
   */
  public readonly _emit: (topic: string, data: any) => Promise<void>

  /**
   * VM is run in DEBUG mode (default: false)
   * Taken from DEBUG environment variable
   *
   * Safeguards on debug() calls are added for
   * performance reasons to avoid string literal evaluation
   * @hidden
   */
  readonly DEBUG: boolean = false

  /**
   * VM async constructor. Creates engine instance and initializes it.
   *
   * @param opts VM engine constructor options
   */
  static async create(opts: VMOpts = {}): Promise<VM> {
    const vm = new this(opts)
    await vm.init()
    return vm
  }

  /**
   * Instantiates a new {@link VM} Object.
   *
   * @deprecated The direct usage of this constructor is discouraged since
   * non-finalized async initialization might lead to side effects. Please
   * use the async {@link VM.create} constructor instead (same API).
   * @param opts
   */
  protected constructor(opts: VMOpts = {}) {
    this.events = new AsyncEventEmitter<VMEvents>()

    this._opts = opts

    if (opts.common) {
      this._common = opts.common
    } else {
      const DEFAULT_CHAIN = Chain.Mainnet
      this._common = new Common({ chain: DEFAULT_CHAIN })
    }

    if (opts.stateManager) {
      this.stateManager = opts.stateManager
    } else {
      this.stateManager = new DefaultStateManager({})
    }

    this.blockchain = opts.blockchain ?? new (Blockchain as any)({ common: this._common })

    // TODO tests
    if (opts.eei) {
      if (opts.evm) {
        throw new Error('cannot specify EEI if EVM opt provided')
      }
      this.eei = opts.eei
    } else {
      if (opts.evm) {
        this.eei = opts.evm.eei
      } else {
        this.eei = new EEI(this.stateManager, this._common, this.blockchain)
      }
    }

    // TODO tests
    if (opts.evm) {
      this.evm = opts.evm
    } else {
      this.evm = new EVM({
        common: this._common,
        eei: this.eei,
      })
    }

    if (opts.hardforkByBlockNumber !== undefined && opts.hardforkByTTD !== undefined) {
      throw new Error(
        `The hardforkByBlockNumber and hardforkByTTD options can't be used in conjunction`
      )
    }

    this._hardforkByBlockNumber = opts.hardforkByBlockNumber ?? false
    this._hardforkByTTD = toType(opts.hardforkByTTD, TypeOutput.BigInt)

    // Safeguard if "process" is not available (browser)
    if (process !== undefined && typeof process.env.DEBUG !== 'undefined') {
      this.DEBUG = true
    }

    // We cache this promisified function as it's called from the main execution loop, and
    // promisifying each time has a huge performance impact.
    this._emit = <(topic: string, data: any) => Promise<void>>(
      promisify(this.events.emit.bind(this.events))
    )
  }

  async init(): Promise<void> {
    if (this._isInitialized) return
    if (typeof (<any>this.blockchain)._init === 'function') {
      await (this.blockchain as any)._init()
    }

    if (!this._opts.stateManager) {
      if (this._opts.activateGenesisState === true) {
        if (typeof (<any>this.blockchain).genesisState === 'function') {
          await this.eei.generateCanonicalGenesis((<any>this.blockchain).genesisState())
        } else {
          throw new Error(
            'cannot activate genesis state: blockchain object has no `genesisState` method'
          )
        }
      }
    }

    if (this._opts.activatePrecompiles === true && typeof this._opts.stateManager === 'undefined') {
      await this.eei.checkpoint()
      // put 1 wei in each of the precompiles in order to make the accounts non-empty and thus not have them deduct `callNewAccount` gas.
      for (const [addressStr] of getActivePrecompiles(this._common)) {
        const address = new Address(Buffer.from(addressStr, 'hex'))
        const account = await this.eei.getAccount(address)
        // Only do this if it is not overridden in genesis
        // Note: in the case that custom genesis has storage fields, this is preserved
        if (account.isEmpty()) {
          const newAccount = Account.fromAccountData({
            balance: 1,
            storageRoot: account.storageRoot,
          })
          await this.eei.putAccount(address, newAccount)
        }
      }
      await this.eei.commit()
    }
    this._isInitialized = true
  }

  /**
   * Processes the `block` running all of the transactions it contains and updating the miner's account
   *
   * This method modifies the state. If `generate` is `true`, the state modifications will be
   * reverted if an exception is raised. If it's `false`, it won't revert if the block's header is
   * invalid. If an error is thrown from an event handler, the state may or may not be reverted.
   *
   * @param {RunBlockOpts} opts - Default values for options:
   *  - `generate`: false
   */
  async runBlock(opts: RunBlockOpts): Promise<RunBlockResult> {
    return runBlock.bind(this)(opts)
  }

  /**
   * Process a transaction. Run the vm. Transfers eth. Checks balances.
   *
   * This method modifies the state. If an error is thrown, the modifications are reverted, except
   * when the error is thrown from an event handler. In the latter case the state may or may not be
   * reverted.
   *
   * @param {RunTxOpts} opts
   */
  async runTx(opts: RunTxOpts): Promise<RunTxResult> {
    return runTx.bind(this)(opts)
  }

  /**
   * Build a block on top of the current state
   * by adding one transaction at a time.
   *
   * Creates a checkpoint on the StateManager and modifies the state
   * as transactions are run. The checkpoint is committed on {@link BlockBuilder.build}
   * or discarded with {@link BlockBuilder.revert}.
   *
   * @param {BuildBlockOpts} opts
   * @returns An instance of {@link BlockBuilder} with methods:
   * - {@link BlockBuilder.addTransaction}
   * - {@link BlockBuilder.build}
   * - {@link BlockBuilder.revert}
   */
  async buildBlock(opts: BuildBlockOpts): Promise<BlockBuilder> {
    return buildBlock.bind(this)(opts)
  }

  /**
   * Returns a copy of the {@link VM} instance.
   */
  async copy(): Promise<VM> {
    const evmCopy = this.evm.copy()
    const eeiCopy: EEIInterface = evmCopy.eei
    return VM.create({
      stateManager: (eeiCopy as any)._stateManager,
      blockchain: (eeiCopy as any)._blockchain,
      common: (eeiCopy as any)._common,
      evm: evmCopy,
      hardforkByBlockNumber: this._hardforkByBlockNumber ? true : undefined,
      hardforkByTTD: this._hardforkByTTD,
    })
  }

  /**
   * Return a compact error string representation of the object
   */
  errorStr() {
    let hf = ''
    try {
      hf = this._common.hardfork()
    } catch (e: any) {
      hf = 'error'
    }
    const errorStr = `vm hf=${hf}`
    return errorStr
  }
}
