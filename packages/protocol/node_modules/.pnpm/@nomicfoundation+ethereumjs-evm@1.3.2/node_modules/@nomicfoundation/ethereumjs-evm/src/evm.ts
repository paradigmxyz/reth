import { Chain, Common, Hardfork } from '@nomicfoundation/ethereumjs-common'
import {
  Address,
  KECCAK256_NULL,
  MAX_INTEGER,
  bigIntToBuffer,
  generateAddress,
  generateAddress2,
  short,
  zeros,
} from '@nomicfoundation/ethereumjs-util'
import AsyncEventEmitter = require('async-eventemitter')
import { debug as createDebugLogger } from 'debug'
import { promisify } from 'util'

import { EOF } from './eof'
import { ERROR, EvmError } from './exceptions'
import { Interpreter } from './interpreter'
import { Message } from './message'
import { getOpcodesForHF } from './opcodes'
import { getActivePrecompiles } from './precompiles'
import { TransientStorage } from './transientStorage'

import type { InterpreterOpts, RunState } from './interpreter'
import type { MessageWithTo } from './message'
import type { OpHandler, OpcodeList } from './opcodes'
import type { AsyncDynamicGasHandler, SyncDynamicGasHandler } from './opcodes/gas'
import type { CustomPrecompile, PrecompileFunc } from './precompiles'
import type {
  Block,
  CustomOpcode,
  EEIInterface,
  EVMEvents,
  EVMInterface,
  EVMRunCallOpts,
  EVMRunCodeOpts,
  /*ExternalInterface,*/
  /*ExternalInterfaceFactory,*/
  Log,
} from './types'
import type { Account } from '@nomicfoundation/ethereumjs-util'

const debug = createDebugLogger('evm')
const debugGas = createDebugLogger('evm:gas')

// very ugly way to detect if we are running in a browser
const isBrowser = new Function('try {return this===window;}catch(e){ return false;}')
let mcl: any
let mclInitPromise: any

if (isBrowser() === false) {
  mcl = require('mcl-wasm')
  mclInitPromise = mcl.init(mcl.BLS12_381)
}

/**
 * Options for instantiating a {@link EVM}.
 */
export interface EVMOpts {
  /**
   * Use a {@link Common} instance for EVM instantiation.
   *
   * ### Supported EIPs
   *
   * - [EIP-1153](https://eips.ethereum.org/EIPS/eip-1153) - Transient Storage Opcodes (`experimental`)
   * - [EIP-1559](https://eips.ethereum.org/EIPS/eip-1559) - EIP-1559 Fee Market
   * - [EIP-2315](https://eips.ethereum.org/EIPS/eip-2315) - VM simple subroutines (`experimental`)
   * - [EIP-2537](https://eips.ethereum.org/EIPS/eip-2537) - BLS12-381 precompiles (`experimental`)
   * - [EIP-2565](https://eips.ethereum.org/EIPS/eip-2565) - ModExp Gas Cost
   * - [EIP-2718](https://eips.ethereum.org/EIPS/eip-2718) - Typed Transactions
   * - [EIP-2929](https://eips.ethereum.org/EIPS/eip-2929) - Gas cost increases for state access opcodes
   * - [EIP-2930](https://eips.ethereum.org/EIPS/eip-2930) - Access List Transaction Type
   * - [EIP-3198](https://eips.ethereum.org/EIPS/eip-3198) - BASEFEE opcode
   * - [EIP-3529](https://eips.ethereum.org/EIPS/eip-3529) - Reduction in refunds
   * - [EIP-3540](https://eips.ethereum.org/EIPS/eip-3541) - EVM Object Format (EOF) v1 (`experimental`)
   * - [EIP-3541](https://eips.ethereum.org/EIPS/eip-3541) - Reject new contracts starting with the 0xEF byte
   *   [EIP-3651](https://eips.ethereum.org/EIPS/eip-3651) - Warm COINBASE (`experimental`)
   * - [EIP-3670](https://eips.ethereum.org/EIPS/eip-3670) - EOF - Code Validation (`experimental`)
   * - [EIP-3855](https://eips.ethereum.org/EIPS/eip-3855) - PUSH0 instruction (`experimental`)
   * - [EIP-3860](https://eips.ethereum.org/EIPS/eip-3860) - Limit and meter initcode (`experimental`)
   * - [EIP-4399](https://eips.ethereum.org/EIPS/eip-4399) - Supplant DIFFICULTY opcode with PREVRANDAO (Merge) (`experimental`)
   * - [EIP-5133](https://eips.ethereum.org/EIPS/eip-5133) - Delaying Difficulty Bomb to mid-September 2022
   *
   * *Annotations:*
   *
   * - `experimental`: behaviour can change on patch versions
   */
  common?: Common

  /**
   * Allows unlimited contract sizes while debugging. By setting this to `true`, the check for
   * contract size limit of 24KB (see [EIP-170](https://git.io/vxZkK)) is bypassed.
   *
   * Default: `false` [ONLY set to `true` during debugging]
   */
  allowUnlimitedContractSize?: boolean

  /**
   * Override or add custom opcodes to the EVM instruction set
   * These custom opcodes are EIP-agnostic and are always statically added
   * To delete an opcode, add an entry of format `{opcode: number}`. This will delete that opcode from the EVM.
   * If this opcode is then used in the EVM, the `INVALID` opcode would instead be used.
   * To add an opcode, add an entry of the following format:
   * {
   *    // The opcode number which will invoke the custom opcode logic
   *    opcode: number
   *    // The name of the opcode (as seen in the `step` event)
   *    opcodeName: string
   *    // The base fee of the opcode
   *    baseFee: number
   *    // If the opcode charges dynamic gas, add this here. To charge the gas, use the `i` methods of the BN, to update the charged gas
   *    gasFunction?: function(runState: RunState, gas: BN, common: Common)
   *    // The logic of the opcode which holds the logic of changing the current state
   *    logicFunction: function(runState: RunState)
   * }
   * Note: gasFunction and logicFunction can both be async or synchronous functions
   */
  customOpcodes?: CustomOpcode[]

  /*
   * Adds custom precompiles. This is hardfork-agnostic: these precompiles are always activated
   * If only an address is given, the precompile is deleted
   * If an address and a `PrecompileFunc` is given, this precompile is inserted or overridden
   * Please ensure `PrecompileFunc` has exactly one parameter `input: PrecompileInput`
   */
  customPrecompiles?: CustomPrecompile[]

  /*
   * The External Interface Factory, used to build an External Interface when this is necessary
   */
  eei: EEIInterface
}

/**
 * EVM is responsible for executing an EVM message fully
 * (including any nested calls and creates), processing the results
 * and storing them to state (or discarding changes in case of exceptions).
 * @ignore
 */
export class EVM implements EVMInterface {
  protected _tx?: {
    gasPrice: bigint
    origin: Address
  }
  protected _block?: Block

  readonly _common: Common

  public eei: EEIInterface

  public readonly _transientStorage: TransientStorage

  public readonly events: AsyncEventEmitter<EVMEvents>

  /**
   * This opcode data is always set since `getActiveOpcodes()` is called in the constructor
   * @hidden
   */
  _opcodes!: OpcodeList

  public readonly _allowUnlimitedContractSize: boolean

  protected readonly _customOpcodes?: CustomOpcode[]
  protected readonly _customPrecompiles?: CustomPrecompile[]

  /**
   * @hidden
   */
  _handlers!: Map<number, OpHandler>

  /**
   * @hidden
   */
  _dynamicGasHandlers!: Map<number, AsyncDynamicGasHandler | SyncDynamicGasHandler>

  protected _precompiles!: Map<string, PrecompileFunc>

  protected readonly _optsCached: EVMOpts

  public get precompiles() {
    return this._precompiles
  }

  public get opcodes() {
    return this._opcodes
  }

  protected _isInitialized: boolean = false

  /**
   * Cached emit() function, not for public usage
   * set to public due to implementation internals
   * @hidden
   */
  public readonly _emit: (topic: string, data: any) => Promise<void>

  /**
   * Pointer to the mcl package, not for public usage
   * set to public due to implementation internals
   * @hidden
   */
  public readonly _mcl: any //

  /**
   * EVM is run in DEBUG mode (default: false)
   * Taken from DEBUG environment variable
   *
   * Safeguards on debug() calls are added for
   * performance reasons to avoid string literal evaluation
   * @hidden
   */
  readonly DEBUG: boolean = false

  /**
   * EVM async constructor. Creates engine instance and initializes it.
   *
   * @param opts EVM engine constructor options
   */
  static async create(opts: EVMOpts): Promise<EVM> {
    const evm = new this(opts)
    await evm.init()
    return evm
  }

  constructor(opts: EVMOpts) {
    this.events = new AsyncEventEmitter<EVMEvents>()

    this._optsCached = opts

    this.eei = opts.eei

    this._transientStorage = new TransientStorage()

    if (opts.common) {
      this._common = opts.common
    } else {
      const DEFAULT_CHAIN = Chain.Mainnet
      this._common = new Common({ chain: DEFAULT_CHAIN })
    }

    // Supported EIPs
    const supportedEIPs = [
      1153, 1559, 2315, 2537, 2565, 2718, 2929, 2930, 3074, 3198, 3529, 3540, 3541, 3607, 3651,
      3670, 3855, 3860, 4399, 5133,
    ]

    for (const eip of this._common.eips()) {
      if (!supportedEIPs.includes(eip)) {
        throw new Error(`EIP-${eip} is not supported by the EVM`)
      }
    }

    const supportedHardforks = [
      Hardfork.Chainstart,
      Hardfork.Homestead,
      Hardfork.Dao,
      Hardfork.TangerineWhistle,
      Hardfork.SpuriousDragon,
      Hardfork.Byzantium,
      Hardfork.Constantinople,
      Hardfork.Petersburg,
      Hardfork.Istanbul,
      Hardfork.MuirGlacier,
      Hardfork.Berlin,
      Hardfork.London,
      Hardfork.ArrowGlacier,
      Hardfork.GrayGlacier,
      Hardfork.MergeForkIdTransition,
      Hardfork.Merge,
    ]
    if (!supportedHardforks.includes(this._common.hardfork() as Hardfork)) {
      throw new Error(
        `Hardfork ${this._common.hardfork()} not set as supported in supportedHardforks`
      )
    }

    this._allowUnlimitedContractSize = opts.allowUnlimitedContractSize ?? false
    this._customOpcodes = opts.customOpcodes
    this._customPrecompiles = opts.customPrecompiles

    this._common.on('hardforkChanged', () => {
      this.getActiveOpcodes()
      this._precompiles = getActivePrecompiles(this._common, this._customPrecompiles)
    })

    // Initialize the opcode data
    this.getActiveOpcodes()
    this._precompiles = getActivePrecompiles(this._common, this._customPrecompiles)

    if (this._common.isActivatedEIP(2537)) {
      if (isBrowser() === true) {
        throw new Error('EIP-2537 is currently not supported in browsers')
      } else {
        this._mcl = mcl
      }
    }

    // Safeguard if "process" is not available (browser)
    if (typeof process?.env.DEBUG !== 'undefined') {
      this.DEBUG = true
    }

    // We cache this promisified function as it's called from the main execution loop, and
    // promisifying each time has a huge performance impact.
    this._emit = <(topic: string, data: any) => Promise<void>>(
      promisify(this.events.emit.bind(this.events))
    )
  }

  protected async init(): Promise<void> {
    if (this._isInitialized) {
      return
    }

    if (this._common.isActivatedEIP(2537)) {
      if (isBrowser() === true) {
        throw new Error('EIP-2537 is currently not supported in browsers')
      } else {
        const mcl = this._mcl
        await mclInitPromise // ensure that mcl is initialized.
        mcl.setMapToMode(mcl.IRTF) // set the right map mode; otherwise mapToG2 will return wrong values.
        mcl.verifyOrderG1(1) // subgroup checks for G1
        mcl.verifyOrderG2(1) // subgroup checks for G2
      }
    }

    this._isInitialized = true
  }

  /**
   * Returns a list with the currently activated opcodes
   * available for EVM execution
   */
  getActiveOpcodes(): OpcodeList {
    const data = getOpcodesForHF(this._common, this._customOpcodes)
    this._opcodes = data.opcodes
    this._dynamicGasHandlers = data.dynamicGasHandlers
    this._handlers = data.handlers
    return data.opcodes
  }

  protected async _executeCall(message: MessageWithTo): Promise<EVMResult> {
    const account = await this.eei.getAccount(message.authcallOrigin ?? message.caller)
    let errorMessage
    // Reduce tx value from sender
    if (!message.delegatecall) {
      try {
        await this._reduceSenderBalance(account, message)
      } catch (e) {
        errorMessage = e
      }
    }
    // Load `to` account
    const toAccount = await this.eei.getAccount(message.to)
    // Add tx value to the `to` account
    if (!message.delegatecall) {
      try {
        await this._addToBalance(toAccount, message)
      } catch (e: any) {
        errorMessage = e
      }
    }

    // Load code
    await this._loadCode(message)
    let exit = false
    if (!message.code || message.code.length === 0) {
      exit = true
      if (this.DEBUG) {
        debug(`Exit early on no code`)
      }
    }
    if (errorMessage !== undefined) {
      exit = true
      if (this.DEBUG) {
        debug(`Exit early on value transfer overflowed`)
      }
    }
    if (exit) {
      return {
        execResult: {
          gasRefund: message.gasRefund,
          executionGasUsed: BigInt(0),
          exceptionError: errorMessage, // Only defined if addToBalance failed
          returnValue: Buffer.alloc(0),
        },
      }
    }

    let result: ExecResult
    if (message.isCompiled) {
      if (this.DEBUG) {
        debug(`Run precompile`)
      }
      result = await this.runPrecompile(
        message.code as PrecompileFunc,
        message.data,
        message.gasLimit
      )
      result.gasRefund = message.gasRefund
    } else {
      if (this.DEBUG) {
        debug(`Start bytecode processing...`)
      }
      result = await this.runInterpreter(message)
    }

    if (message.depth === 0) {
      this.postMessageCleanup()
    }

    return {
      execResult: result,
    }
  }

  protected async _executeCreate(message: Message): Promise<EVMResult> {
    const account = await this.eei.getAccount(message.caller)
    // Reduce tx value from sender
    await this._reduceSenderBalance(account, message)

    if (this._common.isActivatedEIP(3860)) {
      if (message.data.length > Number(this._common.param('vm', 'maxInitCodeSize'))) {
        return {
          createdAddress: message.to,
          execResult: {
            returnValue: Buffer.alloc(0),
            exceptionError: new EvmError(ERROR.INITCODE_SIZE_VIOLATION),
            executionGasUsed: message.gasLimit,
          },
        }
      }
    }

    message.code = message.data
    message.data = Buffer.alloc(0)
    message.to = await this._generateAddress(message)
    if (this.DEBUG) {
      debug(`Generated CREATE contract address ${message.to}`)
    }
    let toAccount = await this.eei.getAccount(message.to)

    // Check for collision
    if (
      (toAccount.nonce && toAccount.nonce > BigInt(0)) ||
      !toAccount.codeHash.equals(KECCAK256_NULL)
    ) {
      if (this.DEBUG) {
        debug(`Returning on address collision`)
      }
      return {
        createdAddress: message.to,
        execResult: {
          returnValue: Buffer.alloc(0),
          exceptionError: new EvmError(ERROR.CREATE_COLLISION),
          executionGasUsed: message.gasLimit,
        },
      }
    }

    await this.eei.clearContractStorage(message.to)

    const newContractEvent = {
      address: message.to,
      code: message.code,
    }

    await this._emit('newContract', newContractEvent)

    toAccount = await this.eei.getAccount(message.to)
    // EIP-161 on account creation and CREATE execution
    if (this._common.gteHardfork(Hardfork.SpuriousDragon)) {
      toAccount.nonce += BigInt(1)
    }

    // Add tx value to the `to` account
    let errorMessage
    try {
      await this._addToBalance(toAccount, message as MessageWithTo)
    } catch (e: any) {
      errorMessage = e
    }

    let exit = false
    if (message.code === undefined || message.code.length === 0) {
      exit = true
      if (this.DEBUG) {
        debug(`Exit early on no code`)
      }
    }
    if (errorMessage !== undefined) {
      exit = true
      if (this.DEBUG) {
        debug(`Exit early on value transfer overflowed`)
      }
    }
    if (exit) {
      return {
        createdAddress: message.to,
        execResult: {
          executionGasUsed: BigInt(0),
          gasRefund: message.gasRefund,
          exceptionError: errorMessage, // only defined if addToBalance failed
          returnValue: Buffer.alloc(0),
        },
      }
    }

    if (this.DEBUG) {
      debug(`Start bytecode processing...`)
    }

    let result = await this.runInterpreter(message)
    // fee for size of the return value
    let totalGas = result.executionGasUsed
    let returnFee = BigInt(0)
    if (!result.exceptionError) {
      returnFee =
        BigInt(result.returnValue.length) * BigInt(this._common.param('gasPrices', 'createData'))
      totalGas = totalGas + returnFee
      if (this.DEBUG) {
        debugGas(`Add return value size fee (${returnFee} to gas used (-> ${totalGas}))`)
      }
    }

    // Check for SpuriousDragon EIP-170 code size limit
    let allowedCodeSize = true
    if (
      !result.exceptionError &&
      this._common.gteHardfork(Hardfork.SpuriousDragon) &&
      result.returnValue.length > Number(this._common.param('vm', 'maxCodeSize'))
    ) {
      allowedCodeSize = false
    }

    // If enough gas and allowed code size
    let CodestoreOOG = false
    if (totalGas <= message.gasLimit && (this._allowUnlimitedContractSize || allowedCodeSize)) {
      if (this._common.isActivatedEIP(3541) && result.returnValue[0] === EOF.FORMAT) {
        if (!this._common.isActivatedEIP(3540)) {
          result = { ...result, ...INVALID_BYTECODE_RESULT(message.gasLimit) }
        }
        // Begin EOF1 contract code checks
        // EIP-3540 EOF1 header check
        const eof1CodeAnalysisResults = EOF.codeAnalysis(result.returnValue)
        if (typeof eof1CodeAnalysisResults?.code === 'undefined') {
          result = {
            ...result,
            ...INVALID_EOF_RESULT(message.gasLimit),
          }
        } else if (this._common.isActivatedEIP(3670)) {
          // EIP-3670 EOF1 opcode check
          const codeStart = eof1CodeAnalysisResults.data > 0 ? 10 : 7
          // The start of the code section of an EOF1 compliant contract will either be
          // index 7 (if no data section is present) or index 10 (if a data section is present)
          // in the bytecode of the contract
          if (
            !EOF.validOpcodes(
              result.returnValue.slice(codeStart, codeStart + eof1CodeAnalysisResults.code)
            )
          ) {
            result = {
              ...result,
              ...INVALID_EOF_RESULT(message.gasLimit),
            }
          } else {
            result.executionGasUsed = totalGas
          }
        }
      } else {
        result.executionGasUsed = totalGas
      }
    } else {
      if (this._common.gteHardfork(Hardfork.Homestead)) {
        if (this.DEBUG) {
          debug(`Not enough gas or code size not allowed (>= Homestead)`)
        }
        result = { ...result, ...CodesizeExceedsMaximumError(message.gasLimit) }
      } else {
        // we are in Frontier
        if (this.DEBUG) {
          debug(`Not enough gas or code size not allowed (Frontier)`)
        }
        if (totalGas - returnFee <= message.gasLimit) {
          // we cannot pay the code deposit fee (but the deposit code actually did run)
          result = { ...result, ...COOGResult(totalGas - returnFee) }
          CodestoreOOG = true
        } else {
          result = { ...result, ...OOGResult(message.gasLimit) }
        }
      }
    }

    // Save code if a new contract was created
    if (
      !result.exceptionError &&
      result.returnValue !== undefined &&
      result.returnValue.length !== 0
    ) {
      await this.eei.putContractCode(message.to, result.returnValue)
      if (this.DEBUG) {
        debug(`Code saved on new contract creation`)
      }
    } else if (CodestoreOOG) {
      // This only happens at Frontier. But, let's do a sanity check;
      if (!this._common.gteHardfork(Hardfork.Homestead)) {
        // Pre-Homestead behavior; put an empty contract.
        // This contract would be considered "DEAD" in later hard forks.
        // It is thus an unecessary default item, which we have to save to dik
        // It does change the state root, but it only wastes storage.
        //await this._state.putContractCode(message.to, result.returnValue)
        const account = await this.eei.getAccount(message.to)
        await this.eei.putAccount(message.to, account)
      }
    }

    return {
      createdAddress: message.to,
      execResult: result,
    }
  }

  /**
   * Starts the actual bytecode processing for a CALL or CREATE, providing
   * it with the {@link EEI}.
   */
  protected async runInterpreter(
    message: Message,
    opts: InterpreterOpts = {}
  ): Promise<ExecResult> {
    const env = {
      address: message.to ?? Address.zero(),
      caller: message.caller ?? Address.zero(),
      callData: message.data ?? Buffer.from([0]),
      callValue: message.value ?? BigInt(0),
      code: message.code as Buffer,
      isStatic: message.isStatic ?? false,
      depth: message.depth ?? 0,
      gasPrice: this._tx!.gasPrice,
      origin: this._tx!.origin ?? message.caller ?? Address.zero(),
      block: this._block ?? defaultBlock(),
      contract: await this.eei.getAccount(message.to ?? Address.zero()),
      codeAddress: message.codeAddress,
      gasRefund: message.gasRefund,
    }

    const interpreter = new Interpreter(this, this.eei, env, message.gasLimit)
    if (message.selfdestruct) {
      interpreter._result.selfdestruct = message.selfdestruct as { [key: string]: Buffer }
    }

    const interpreterRes = await interpreter.run(message.code as Buffer, opts)

    let result = interpreter._result
    let gasUsed = message.gasLimit - interpreterRes.runState!.gasLeft
    if (interpreterRes.exceptionError) {
      if (
        interpreterRes.exceptionError.error !== ERROR.REVERT &&
        interpreterRes.exceptionError.error !== ERROR.INVALID_EOF_FORMAT
      ) {
        gasUsed = message.gasLimit
      }

      // Clear the result on error
      result = {
        ...result,
        logs: [],
        selfdestruct: {},
      }
    }

    return {
      ...result,
      runState: {
        ...interpreterRes.runState!,
        ...result,
        ...interpreter._env,
      },
      exceptionError: interpreterRes.exceptionError,
      gas: interpreterRes.runState?.gasLeft,
      executionGasUsed: gasUsed,
      gasRefund: interpreterRes.runState!.gasRefund,
      returnValue: result.returnValue ? result.returnValue : Buffer.alloc(0),
    }
  }

  /**
   * Executes an EVM message, determining whether it's a call or create
   * based on the `to` address. It checkpoints the state and reverts changes
   * if an exception happens during the message execution.
   */
  async runCall(opts: EVMRunCallOpts): Promise<EVMResult> {
    let message = opts.message
    if (!message) {
      this._block = opts.block ?? defaultBlock()
      this._tx = {
        gasPrice: opts.gasPrice ?? BigInt(0),
        origin: opts.origin ?? opts.caller ?? Address.zero(),
      }

      const caller = opts.caller ?? Address.zero()

      const value = opts.value ?? BigInt(0)
      if (opts.skipBalance === true) {
        const callerAccount = await this.eei.getAccount(caller)
        if (callerAccount.balance < value) {
          // if skipBalance and balance less than value, set caller balance to `value` to ensure sufficient funds
          callerAccount.balance = value
          await this.eei.putAccount(caller, callerAccount)
        }
      }

      message = new Message({
        caller,
        gasLimit: opts.gasLimit ?? BigInt(0xffffff),
        to: opts.to,
        value,
        data: opts.data,
        code: opts.code,
        depth: opts.depth,
        isCompiled: opts.isCompiled,
        isStatic: opts.isStatic,
        salt: opts.salt,
        selfdestruct: opts.selfdestruct ?? {},
        delegatecall: opts.delegatecall,
      })
    }

    await this._emit('beforeMessage', message)

    if (!message.to && this._common.isActivatedEIP(2929) === true) {
      message.code = message.data
      this.eei.addWarmedAddress((await this._generateAddress(message)).buf)
    }

    await this.eei.checkpoint()
    this._transientStorage.checkpoint()
    if (this.DEBUG) {
      debug('-'.repeat(100))
      debug(`message checkpoint`)
    }

    let result
    if (this.DEBUG) {
      const { caller, gasLimit, to, value, delegatecall } = message
      debug(
        `New message caller=${caller} gasLimit=${gasLimit} to=${
          to?.toString() ?? 'none'
        } value=${value} delegatecall=${delegatecall ? 'yes' : 'no'}`
      )
    }
    if (message.to) {
      if (this.DEBUG) {
        debug(`Message CALL execution (to: ${message.to})`)
      }
      result = await this._executeCall(message as MessageWithTo)
    } else {
      if (this.DEBUG) {
        debug(`Message CREATE execution (to undefined)`)
      }
      result = await this._executeCreate(message)
    }
    if (this.DEBUG) {
      const { executionGasUsed, exceptionError, returnValue } = result.execResult
      debug(
        `Received message execResult: [ gasUsed=${executionGasUsed} exceptionError=${
          exceptionError ? `'${exceptionError.error}'` : 'none'
        } returnValue=0x${short(returnValue)} gasRefund=${result.execResult.gasRefund ?? 0} ]`
      )
    }
    const err = result.execResult.exceptionError
    // This clause captures any error which happened during execution
    // If that is the case, then all refunds are forfeited
    if (err) {
      result.execResult.selfdestruct = {}
      result.execResult.gasRefund = BigInt(0)
    }
    if (err) {
      if (
        this._common.gteHardfork(Hardfork.Homestead) ||
        err.error !== ERROR.CODESTORE_OUT_OF_GAS
      ) {
        result.execResult.logs = []
        await this.eei.revert()
        this._transientStorage.revert()
        if (this.DEBUG) {
          debug(`message checkpoint reverted`)
        }
      } else {
        // we are in chainstart and the error was the code deposit error
        // we do like nothing happened.
        await this.eei.commit()
        this._transientStorage.commit()
        if (this.DEBUG) {
          debug(`message checkpoint committed`)
        }
      }
    } else {
      await this.eei.commit()
      this._transientStorage.commit()
      if (this.DEBUG) {
        debug(`message checkpoint committed`)
      }
    }
    await this._emit('afterMessage', result)

    return result
  }

  /**
   * Bound to the global VM and therefore
   * shouldn't be used directly from the evm class
   */
  async runCode(opts: EVMRunCodeOpts): Promise<ExecResult> {
    this._block = opts.block ?? defaultBlock()

    this._tx = {
      gasPrice: opts.gasPrice ?? BigInt(0),
      origin: opts.origin ?? opts.caller ?? Address.zero(),
    }

    const message = new Message({
      code: opts.code,
      data: opts.data,
      gasLimit: opts.gasLimit,
      to: opts.address ?? Address.zero(),
      caller: opts.caller,
      value: opts.value,
      depth: opts.depth,
      selfdestruct: opts.selfdestruct ?? {},
      isStatic: opts.isStatic,
    })

    return this.runInterpreter(message, { pc: opts.pc })
  }

  /**
   * Returns code for precompile at the given address, or undefined
   * if no such precompile exists.
   */
  getPrecompile(address: Address): PrecompileFunc | undefined {
    return this.precompiles.get(address.buf.toString('hex'))
  }

  /**
   * Executes a precompiled contract with given data and gas limit.
   */
  protected runPrecompile(
    code: PrecompileFunc,
    data: Buffer,
    gasLimit: bigint
  ): Promise<ExecResult> | ExecResult {
    if (typeof code !== 'function') {
      throw new Error('Invalid precompile')
    }

    const opts = {
      data,
      gasLimit,
      _common: this._common,
      _EVM: this,
    }

    return code(opts)
  }

  protected async _loadCode(message: Message): Promise<void> {
    if (!message.code) {
      const precompile = this.getPrecompile(message.codeAddress)
      if (precompile) {
        message.code = precompile
        message.isCompiled = true
      } else {
        message.code = await this.eei.getContractCode(message.codeAddress)
        message.isCompiled = false
      }
    }
  }

  protected async _generateAddress(message: Message): Promise<Address> {
    let addr
    if (message.salt) {
      addr = generateAddress2(message.caller.buf, message.salt, message.code as Buffer)
    } else {
      const acc = await this.eei.getAccount(message.caller)
      let newNonce = acc.nonce
      if (message.depth > 0) {
        newNonce--
      }
      addr = generateAddress(message.caller.buf, bigIntToBuffer(newNonce))
    }
    return new Address(addr)
  }

  protected async _reduceSenderBalance(account: Account, message: Message): Promise<void> {
    account.balance -= message.value
    if (account.balance < BigInt(0)) {
      throw new EvmError(ERROR.INSUFFICIENT_BALANCE)
    }
    const result = this.eei.putAccount(message.authcallOrigin ?? message.caller, account)
    if (this.DEBUG) {
      debug(`Reduced sender (${message.caller}) balance (-> ${account.balance})`)
    }
    return result
  }

  protected async _addToBalance(toAccount: Account, message: MessageWithTo): Promise<void> {
    const newBalance = toAccount.balance + message.value
    if (newBalance > MAX_INTEGER) {
      throw new EvmError(ERROR.VALUE_OVERFLOW)
    }
    toAccount.balance = newBalance
    // putAccount as the nonce may have changed for contract creation
    const result = this.eei.putAccount(message.to, toAccount)
    if (this.DEBUG) {
      debug(`Added toAccount (${message.to}) balance (-> ${toAccount.balance})`)
    }
    return result
  }

  protected async _touchAccount(address: Address): Promise<void> {
    const account = await this.eei.getAccount(address)
    return this.eei.putAccount(address, account)
  }

  /**
   * Once the interpreter has finished depth 0, a post-message cleanup should be done
   */
  private postMessageCleanup() {
    if (this._common.isActivatedEIP(1153)) this._transientStorage.clear()
  }

  public copy() {
    const opts = {
      ...this._optsCached,
      common: this._common.copy(),
      eei: this.eei.copy(),
    }
    return new EVM(opts)
  }
}

/**
 * Result of executing a message via the {@link EVM}.
 */
export interface EVMResult {
  /**
   * Address of created account during transaction, if any
   */
  createdAddress?: Address
  /**
   * Contains the results from running the code, if any, as described in {@link runCode}
   */
  execResult: ExecResult
}

/**
 * Result of executing a call via the {@link EVM}.
 */
export interface ExecResult {
  runState?: RunState
  /**
   * Description of the exception, if any occurred
   */
  exceptionError?: EvmError
  /**
   * Amount of gas left
   */
  gas?: bigint
  /**
   * Amount of gas the code used to run
   */
  executionGasUsed: bigint
  /**
   * Return value from the contract
   */
  returnValue: Buffer
  /**
   * Array of logs that the contract emitted
   */
  logs?: Log[]
  /**
   * A map from the accounts that have self-destructed to the addresses to send their funds to
   */
  selfdestruct?: { [k: string]: Buffer }
  /**
   * The gas refund counter
   */
  gasRefund?: bigint
}

export function OOGResult(gasLimit: bigint): ExecResult {
  return {
    returnValue: Buffer.alloc(0),
    executionGasUsed: gasLimit,
    exceptionError: new EvmError(ERROR.OUT_OF_GAS),
  }
}
// CodeDeposit OOG Result
export function COOGResult(gasUsedCreateCode: bigint): ExecResult {
  return {
    returnValue: Buffer.alloc(0),
    executionGasUsed: gasUsedCreateCode,
    exceptionError: new EvmError(ERROR.CODESTORE_OUT_OF_GAS),
  }
}

export function INVALID_BYTECODE_RESULT(gasLimit: bigint): ExecResult {
  return {
    returnValue: Buffer.alloc(0),
    executionGasUsed: gasLimit,
    exceptionError: new EvmError(ERROR.INVALID_BYTECODE_RESULT),
  }
}

export function INVALID_EOF_RESULT(gasLimit: bigint): ExecResult {
  return {
    returnValue: Buffer.alloc(0),
    executionGasUsed: gasLimit,
    exceptionError: new EvmError(ERROR.INVALID_EOF_FORMAT),
  }
}

export function CodesizeExceedsMaximumError(gasUsed: bigint): ExecResult {
  return {
    returnValue: Buffer.alloc(0),
    executionGasUsed: gasUsed,
    exceptionError: new EvmError(ERROR.CODESIZE_EXCEEDS_MAXIMUM),
  }
}

export function EvmErrorResult(error: EvmError, gasUsed: bigint): ExecResult {
  return {
    returnValue: Buffer.alloc(0),
    executionGasUsed: gasUsed,
    exceptionError: error,
  }
}

function defaultBlock(): Block {
  return {
    header: {
      number: BigInt(0),
      cliqueSigner: () => Address.zero(),
      coinbase: Address.zero(),
      timestamp: BigInt(0),
      difficulty: BigInt(0),
      prevRandao: zeros(32),
      gasLimit: BigInt(0),
      baseFeePerGas: undefined,
    },
  }
}
