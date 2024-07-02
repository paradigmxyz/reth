import { ConsensusAlgorithm } from '@nomicfoundation/ethereumjs-common'
import { MAX_UINT64, bigIntToHex, bufferToBigInt, intToHex } from '@nomicfoundation/ethereumjs-util'
import { debug as createDebugLogger } from 'debug'

import { EOF } from './eof'
import { ERROR, EvmError } from './exceptions'
import { Memory } from './memory'
import { Message } from './message'
import { trap } from './opcodes'
import { Stack } from './stack'

import type { EVM, EVMResult } from './evm'
import type { AsyncOpHandler, OpHandler, Opcode } from './opcodes'
import type { Block, EEIInterface, Log } from './types'
import type { Common } from '@nomicfoundation/ethereumjs-common'
import type { Account, Address } from '@nomicfoundation/ethereumjs-util'

const debugGas = createDebugLogger('evm:eei:gas')

export interface InterpreterOpts {
  pc?: number
}

/**
 * Immediate (unprocessed) result of running an EVM bytecode.
 */
export interface RunResult {
  logs: Log[]
  returnValue?: Buffer
  /**
   * A map from the accounts that have self-destructed to the addresses to send their funds to
   */
  selfdestruct: { [k: string]: Buffer }
}

export interface Env {
  address: Address
  caller: Address
  callData: Buffer
  callValue: bigint
  code: Buffer
  isStatic: boolean
  depth: number
  gasPrice: bigint
  origin: Address
  block: Block
  contract: Account
  codeAddress: Address /* Different than address for DELEGATECALL and CALLCODE */
  gasRefund: bigint /* Current value (at begin of the frame) of the gas refund */
}

export interface RunState {
  programCounter: number
  opCode: number
  memory: Memory
  memoryWordCount: bigint
  highestMemCost: bigint
  stack: Stack
  returnStack: Stack
  code: Buffer
  shouldDoJumpAnalysis: boolean
  validJumps: Uint8Array // array of values where validJumps[index] has value 0 (default), 1 (jumpdest), 2 (beginsub)
  eei: EEIInterface
  env: Env
  messageGasLimit?: bigint // Cache value from `gas.ts` to save gas limit for a message call
  interpreter: Interpreter
  gasRefund: bigint // Tracks the current refund
  gasLeft: bigint // Current gas left
  auth?: Address /** EIP-3074 AUTH parameter */
  returnBuffer: Buffer /* Current bytes in the return buffer. Cleared each time a CALL/CREATE is made in the current frame. */
}

export interface InterpreterResult {
  runState: RunState
  exceptionError?: EvmError
}

export interface InterpreterStep {
  gasLeft: bigint
  gasRefund: bigint
  eei: EEIInterface
  stack: bigint[]
  returnStack: bigint[]
  pc: number
  depth: number
  opcode: {
    name: string
    fee: number
    dynamicFee?: bigint
    isAsync: boolean
  }
  account: Account
  address: Address
  memory: Buffer
  memoryWordCount: bigint
  codeAddress: Address
}

/**
 * Parses and executes EVM bytecode.
 */
export class Interpreter {
  protected _vm: any
  protected _runState: RunState
  protected _eei: EEIInterface
  protected _common: Common
  protected _evm: EVM
  _env: Env

  // Keep track of this Interpreter run result
  // TODO move into Env?
  _result: RunResult

  // Opcode debuggers (e.g. { 'push': [debug Object], 'sstore': [debug Object], ...})
  private opDebuggers: { [key: string]: (debug: string) => void } = {}

  // TODO remove eei from constructor this can be directly read from EVM
  // EEI gets created on EVM creation and will not be re-instantiated
  // TODO remove gasLeft as constructor argument
  constructor(evm: EVM, eei: EEIInterface, env: Env, gasLeft: bigint) {
    this._evm = evm
    this._eei = eei
    this._common = this._evm._common
    this._runState = {
      programCounter: 0,
      opCode: 0xfe, // INVALID opcode
      memory: new Memory(),
      memoryWordCount: BigInt(0),
      highestMemCost: BigInt(0),
      stack: new Stack(),
      returnStack: new Stack(1023), // 1023 return stack height limit per EIP 2315 spec
      code: Buffer.alloc(0),
      validJumps: Uint8Array.from([]),
      eei: this._eei,
      env,
      shouldDoJumpAnalysis: true,
      interpreter: this,
      gasRefund: env.gasRefund,
      gasLeft,
      returnBuffer: Buffer.alloc(0),
    }
    this._env = env
    this._result = {
      logs: [],
      returnValue: undefined,
      selfdestruct: {},
    }
  }

  async run(code: Buffer, opts: InterpreterOpts = {}): Promise<InterpreterResult> {
    if (!this._common.isActivatedEIP(3540) || code[0] !== EOF.FORMAT) {
      // EIP-3540 isn't active and first byte is not 0xEF - treat as legacy bytecode
      this._runState.code = code
    } else if (this._common.isActivatedEIP(3540)) {
      if (code[1] !== EOF.MAGIC) {
        // Bytecode contains invalid EOF magic byte
        return {
          runState: this._runState,
          exceptionError: new EvmError(ERROR.INVALID_BYTECODE_RESULT),
        }
      }
      if (code[2] !== EOF.VERSION) {
        // Bytecode contains invalid EOF version number
        return {
          runState: this._runState,
          exceptionError: new EvmError(ERROR.INVALID_EOF_FORMAT),
        }
      }
      // Code is EOF1 format
      const codeSections = EOF.codeAnalysis(code)
      if (!codeSections) {
        // Code is invalid EOF1 format if `codeSections` is falsy
        return {
          runState: this._runState,
          exceptionError: new EvmError(ERROR.INVALID_EOF_FORMAT),
        }
      }

      if (codeSections.data) {
        // Set code to EOF container code section which starts at byte position 10 if data section is present
        this._runState.code = code.slice(10, 10 + codeSections!.code)
      } else {
        // Set code to EOF container code section which starts at byte position 7 if no data section is present
        this._runState.code = code.slice(7, 7 + codeSections!.code)
      }
    }
    this._runState.programCounter = opts.pc ?? this._runState.programCounter
    // Check that the programCounter is in range
    const pc = this._runState.programCounter
    if (pc !== 0 && (pc < 0 || pc >= this._runState.code.length)) {
      throw new Error('Internal error: program counter not in range')
    }

    let err
    // Iterate through the given ops until something breaks or we hit STOP
    while (this._runState.programCounter < this._runState.code.length) {
      const opCode = this._runState.code[this._runState.programCounter]
      if (
        this._runState.shouldDoJumpAnalysis &&
        (opCode === 0x56 || opCode === 0x57 || opCode === 0x5e)
      ) {
        // Only run the jump destination analysis if `code` actually contains a JUMP/JUMPI/JUMPSUB opcode
        this._runState.validJumps = this._getValidJumpDests(this._runState.code)
        this._runState.shouldDoJumpAnalysis = false
      }
      this._runState.opCode = opCode

      try {
        await this.runStep()
      } catch (e: any) {
        // re-throw on non-VM errors
        if (!('errorType' in e && e.errorType === 'EvmError')) {
          throw e
        }
        // STOP is not an exception
        if (e.error !== ERROR.STOP) {
          err = e
        }
        break
      }
    }

    return {
      runState: this._runState,
      exceptionError: err,
    }
  }

  /**
   * Executes the opcode to which the program counter is pointing,
   * reducing its base gas cost, and increments the program counter.
   */
  async runStep(): Promise<void> {
    const opInfo = this.lookupOpInfo(this._runState.opCode)

    let gas = BigInt(opInfo.fee)
    // clone the gas limit; call opcodes can add stipend,
    // which makes it seem like the gas left increases
    const gasLimitClone = this.getGasLeft()

    if (opInfo.dynamicGas) {
      const dynamicGasHandler = this._evm._dynamicGasHandlers.get(this._runState.opCode)!
      // This function updates the gas in-place.
      // It needs the base fee, for correct gas limit calculation for the CALL opcodes
      gas = await dynamicGasHandler(this._runState, gas, this._common)
    }

    if (this._evm.events.listenerCount('step') > 0 || this._evm.DEBUG) {
      // Only run this stepHook function if there is an event listener (e.g. test runner)
      // or if the vm is running in debug mode (to display opcode debug logs)
      await this._runStepHook(gas, gasLimitClone)
    }

    // Check for invalid opcode
    if (opInfo.name === 'INVALID') {
      throw new EvmError(ERROR.INVALID_OPCODE)
    }

    // Reduce opcode's base fee
    this.useGas(gas, `${opInfo.name} fee`)
    // Advance program counter
    this._runState.programCounter++

    // Execute opcode handler
    const opFn = this.getOpHandler(opInfo)

    if (opInfo.isAsync) {
      await (opFn as AsyncOpHandler).apply(null, [this._runState, this._common])
    } else {
      opFn.apply(null, [this._runState, this._common])
    }
  }

  /**
   * Get the handler function for an opcode.
   */
  getOpHandler(opInfo: Opcode): OpHandler {
    return this._evm._handlers.get(opInfo.code)!
  }

  /**
   * Get info for an opcode from EVM's list of opcodes.
   */
  lookupOpInfo(op: number): Opcode {
    // if not found, return 0xfe: INVALID
    return this._evm._opcodes.get(op) ?? this._evm._opcodes.get(0xfe)!
  }

  async _runStepHook(dynamicFee: bigint, gasLeft: bigint): Promise<void> {
    const opcode = this.lookupOpInfo(this._runState.opCode)
    const eventObj: InterpreterStep = {
      pc: this._runState.programCounter,
      gasLeft,
      gasRefund: this._runState.gasRefund,
      opcode: {
        name: opcode.fullName,
        fee: opcode.fee,
        dynamicFee,
        isAsync: opcode.isAsync,
      },
      stack: this._runState.stack._store,
      returnStack: this._runState.returnStack._store,
      depth: this._env.depth,
      address: this._env.address,
      account: this._env.contract,
      memory: this._runState.memory._store,
      memoryWordCount: this._runState.memoryWordCount,
      codeAddress: this._env.codeAddress,
      eei: this._runState.eei,
    }

    if (this._evm.DEBUG) {
      // Create opTrace for debug functionality
      let hexStack = []
      hexStack = eventObj.stack.map((item: any) => {
        return bigIntToHex(BigInt(item))
      })

      const name = eventObj.opcode.name

      const opTrace = {
        pc: eventObj.pc,
        op: name,
        gas: bigIntToHex(eventObj.gasLeft),
        gasCost: intToHex(eventObj.opcode.fee),
        stack: hexStack,
        depth: eventObj.depth,
      }

      if (!(name in this.opDebuggers)) {
        this.opDebuggers[name] = createDebugLogger(`evm:ops:${name}`)
      }
      this.opDebuggers[name](JSON.stringify(opTrace))
    }

    /**
     * The `step` event for trace output
     *
     * @event Event: step
     * @type {Object}
     * @property {Number} pc representing the program counter
     * @property {Object} opcode the next opcode to be ran
     * @property {string}     opcode.name
     * @property {fee}        opcode.number Base fee of the opcode
     * @property {dynamicFee} opcode.dynamicFee Dynamic opcode fee
     * @property {boolean}    opcode.isAsync opcode is async
     * @property {BigInt} gasLeft amount of gasLeft
     * @property {BigInt} gasRefund gas refund
     * @property {StateManager} stateManager a {@link StateManager} instance
     * @property {Array} stack an `Array` of `Buffers` containing the stack
     * @property {Array} returnStack the return stack
     * @property {Account} account the Account which owns the code running
     * @property {Address} address the address of the `account`
     * @property {Number} depth the current number of calls deep the contract is
     * @property {Buffer} memory the memory of the EVM as a `buffer`
     * @property {BigInt} memoryWordCount current size of memory in words
     * @property {Address} codeAddress the address of the code which is currently being ran (this differs from `address` in a `DELEGATECALL` and `CALLCODE` call)
     */
    return this._evm._emit('step', eventObj)
  }

  // Returns all valid jump and jumpsub destinations.
  _getValidJumpDests(code: Buffer) {
    const jumps = new Uint8Array(code.length).fill(0)

    for (let i = 0; i < code.length; i++) {
      const opcode = code[i]
      // skip over PUSH0-32 since no jump destinations in the middle of a push block
      if (opcode <= 0x7f) {
        if (opcode >= 0x60) {
          i += opcode - 0x5f
        } else if (opcode === 0x5b) {
          // Define a JUMPDEST as a 1 in the valid jumps array
          jumps[i] = 1
        } else if (opcode === 0x5c) {
          // Define a BEGINSUB as a 2 in the valid jumps array
          jumps[i] = 2
        }
      }
    }
    return jumps
  }

  /**
   * Logic extracted from EEI
   */

  /**
   * Subtracts an amount from the gas counter.
   * @param amount - Amount of gas to consume
   * @param context - Usage context for debugging
   * @throws if out of gas
   */
  useGas(amount: bigint, context?: string): void {
    this._runState.gasLeft -= amount
    if (this._evm.DEBUG) {
      debugGas(
        `${typeof context === 'string' ? context + ': ' : ''}used ${amount} gas (-> ${
          this._runState.gasLeft
        })`
      )
    }
    if (this._runState.gasLeft < BigInt(0)) {
      this._runState.gasLeft = BigInt(0)
      trap(ERROR.OUT_OF_GAS)
    }
  }

  /**
   * Adds a positive amount to the gas counter.
   * @param amount - Amount of gas refunded
   * @param context - Usage context for debugging
   */
  refundGas(amount: bigint, context?: string): void {
    if (this._evm.DEBUG) {
      debugGas(
        `${typeof context === 'string' ? context + ': ' : ''}refund ${amount} gas (-> ${
          this._runState.gasRefund
        })`
      )
    }
    this._runState.gasRefund += amount
  }

  /**
   * Reduces amount of gas to be refunded by a positive value.
   * @param amount - Amount to subtract from gas refunds
   * @param context - Usage context for debugging
   */
  subRefund(amount: bigint, context?: string): void {
    if (this._evm.DEBUG) {
      debugGas(
        `${typeof context === 'string' ? context + ': ' : ''}sub gas refund ${amount} (-> ${
          this._runState.gasRefund
        })`
      )
    }
    this._runState.gasRefund -= amount
    if (this._runState.gasRefund < BigInt(0)) {
      this._runState.gasRefund = BigInt(0)
      trap(ERROR.REFUND_EXHAUSTED)
    }
  }

  /**
   * Increments the internal gasLeft counter. Used for adding callStipend.
   * @param amount - Amount to add
   */
  addStipend(amount: bigint): void {
    if (this._evm.DEBUG) {
      debugGas(`add stipend ${amount} (-> ${this._runState.gasLeft})`)
    }
    this._runState.gasLeft += amount
  }

  /**
   * Returns balance of the given account.
   * @param address - Address of account
   */
  async getExternalBalance(address: Address): Promise<bigint> {
    // shortcut if current account
    if (address.equals(this._env.address)) {
      return this._env.contract.balance
    }

    return (await this._eei.getAccount(address)).balance
  }

  /**
   * Store 256-bit a value in memory to persistent storage.
   */
  async storageStore(key: Buffer, value: Buffer): Promise<void> {
    await this._eei.storageStore(this._env.address, key, value)
    const account = await this._eei.getAccount(this._env.address)
    this._env.contract = account
  }

  /**
   * Loads a 256-bit value to memory from persistent storage.
   * @param key - Storage key
   * @param original - If true, return the original storage value (default: false)
   */
  async storageLoad(key: Buffer, original = false): Promise<Buffer> {
    return this._eei.storageLoad(this._env.address, key, original)
  }

  /**
   * Store 256-bit a value in memory to transient storage.
   * @param address Address to use
   * @param key Storage key
   * @param value Storage value
   */
  transientStorageStore(key: Buffer, value: Buffer): void {
    return this._evm._transientStorage.put(this._env.address, key, value)
  }

  /**
   * Loads a 256-bit value to memory from transient storage.
   * @param address Address to use
   * @param key Storage key
   */
  transientStorageLoad(key: Buffer): Buffer {
    return this._evm._transientStorage.get(this._env.address, key)
  }

  /**
   * Set the returning output data for the execution.
   * @param returnData - Output data to return
   */
  finish(returnData: Buffer): void {
    this._result.returnValue = returnData
    trap(ERROR.STOP)
  }

  /**
   * Set the returning output data for the execution. This will halt the
   * execution immediately and set the execution result to "reverted".
   * @param returnData - Output data to return
   */
  revert(returnData: Buffer): void {
    this._result.returnValue = returnData
    trap(ERROR.REVERT)
  }

  /**
   * Returns address of currently executing account.
   */
  getAddress(): Address {
    return this._env.address
  }

  /**
   * Returns balance of self.
   */
  getSelfBalance(): bigint {
    return this._env.contract.balance
  }

  /**
   * Returns the deposited value by the instruction/transaction
   * responsible for this execution.
   */
  getCallValue(): bigint {
    return this._env.callValue
  }

  /**
   * Returns input data in current environment. This pertains to the input
   * data passed with the message call instruction or transaction.
   */
  getCallData(): Buffer {
    return this._env.callData
  }

  /**
   * Returns size of input data in current environment. This pertains to the
   * input data passed with the message call instruction or transaction.
   */
  getCallDataSize(): bigint {
    return BigInt(this._env.callData.length)
  }

  /**
   * Returns caller address. This is the address of the account
   * that is directly responsible for this execution.
   */
  getCaller(): bigint {
    return bufferToBigInt(this._env.caller.buf)
  }

  /**
   * Returns the size of code running in current environment.
   */
  getCodeSize(): bigint {
    return BigInt(this._env.code.length)
  }

  /**
   * Returns the code running in current environment.
   */
  getCode(): Buffer {
    return this._env.code
  }

  /**
   * Returns the current gasCounter.
   */
  getGasLeft(): bigint {
    return this._runState.gasLeft
  }

  /**
   * Returns size of current return data buffer. This contains the return data
   * from the last executed call, callCode, callDelegate, callStatic or create.
   * Note: create only fills the return data buffer in case of a failure.
   */
  getReturnDataSize(): bigint {
    return BigInt(this._runState.returnBuffer.length)
  }

  /**
   * Returns the current return data buffer. This contains the return data
   * from last executed call, callCode, callDelegate, callStatic or create.
   * Note: create only fills the return data buffer in case of a failure.
   */
  getReturnData(): Buffer {
    return this._runState.returnBuffer
  }

  /**
   * Returns true if the current call must be executed statically.
   */
  isStatic(): boolean {
    return this._env.isStatic
  }

  /**
   * Returns price of gas in current environment.
   */
  getTxGasPrice(): bigint {
    return this._env.gasPrice
  }

  /**
   * Returns the execution's origination address. This is the
   * sender of original transaction; it is never an account with
   * non-empty associated code.
   */
  getTxOrigin(): bigint {
    return bufferToBigInt(this._env.origin.buf)
  }

  /**
   * Returns the block’s number.
   */
  getBlockNumber(): bigint {
    return this._env.block.header.number
  }

  /**
   * Returns the block's beneficiary address.
   */
  getBlockCoinbase(): bigint {
    let coinbase: Address
    if (this._common.consensusAlgorithm() === ConsensusAlgorithm.Clique) {
      coinbase = this._env.block.header.cliqueSigner()
    } else {
      coinbase = this._env.block.header.coinbase
    }
    return bufferToBigInt(coinbase.toBuffer())
  }

  /**
   * Returns the block's timestamp.
   */
  getBlockTimestamp(): bigint {
    return this._env.block.header.timestamp
  }

  /**
   * Returns the block's difficulty.
   */
  getBlockDifficulty(): bigint {
    return this._env.block.header.difficulty
  }

  /**
   * Returns the block's prevRandao field.
   */
  getBlockPrevRandao(): bigint {
    return bufferToBigInt(this._env.block.header.prevRandao)
  }

  /**
   * Returns the block's gas limit.
   */
  getBlockGasLimit(): bigint {
    return this._env.block.header.gasLimit
  }

  /**
   * Returns the Base Fee of the block as proposed in [EIP-3198](https;//eips.etheruem.org/EIPS/eip-3198)
   */
  getBlockBaseFee(): bigint {
    const baseFee = this._env.block.header.baseFeePerGas
    if (baseFee === undefined) {
      // Sanity check
      throw new Error('Block has no Base Fee')
    }
    return baseFee
  }

  /**
   * Returns the chain ID for current chain. Introduced for the
   * CHAINID opcode proposed in [EIP-1344](https://eips.ethereum.org/EIPS/eip-1344).
   */
  getChainId(): bigint {
    return this._common.chainId()
  }

  /**
   * Sends a message with arbitrary data to a given address path.
   */
  async call(gasLimit: bigint, address: Address, value: bigint, data: Buffer): Promise<bigint> {
    const msg = new Message({
      caller: this._env.address,
      gasLimit,
      to: address,
      value,
      data,
      isStatic: this._env.isStatic,
      depth: this._env.depth + 1,
    })

    return this._baseCall(msg)
  }

  /**
   * Sends a message with arbitrary data to a given address path.
   */
  async authcall(gasLimit: bigint, address: Address, value: bigint, data: Buffer): Promise<bigint> {
    const msg = new Message({
      caller: this._runState.auth,
      gasLimit,
      to: address,
      value,
      data,
      isStatic: this._env.isStatic,
      depth: this._env.depth + 1,
      authcallOrigin: this._env.address,
    })

    return this._baseCall(msg)
  }

  /**
   * Message-call into this account with an alternative account's code.
   */
  async callCode(gasLimit: bigint, address: Address, value: bigint, data: Buffer): Promise<bigint> {
    const msg = new Message({
      caller: this._env.address,
      gasLimit,
      to: this._env.address,
      codeAddress: address,
      value,
      data,
      isStatic: this._env.isStatic,
      depth: this._env.depth + 1,
    })

    return this._baseCall(msg)
  }

  /**
   * Sends a message with arbitrary data to a given address path, but disallow
   * state modifications. This includes log, create, selfdestruct and call with
   * a non-zero value.
   */
  async callStatic(
    gasLimit: bigint,
    address: Address,
    value: bigint,
    data: Buffer
  ): Promise<bigint> {
    const msg = new Message({
      caller: this._env.address,
      gasLimit,
      to: address,
      value,
      data,
      isStatic: true,
      depth: this._env.depth + 1,
    })

    return this._baseCall(msg)
  }

  /**
   * Message-call into this account with an alternative account’s code, but
   * persisting the current values for sender and value.
   */
  async callDelegate(
    gasLimit: bigint,
    address: Address,
    value: bigint,
    data: Buffer
  ): Promise<bigint> {
    const msg = new Message({
      caller: this._env.caller,
      gasLimit,
      to: this._env.address,
      codeAddress: address,
      value,
      data,
      isStatic: this._env.isStatic,
      delegatecall: true,
      depth: this._env.depth + 1,
    })

    return this._baseCall(msg)
  }

  async _baseCall(msg: Message): Promise<bigint> {
    const selfdestruct = { ...this._result.selfdestruct }
    msg.selfdestruct = selfdestruct
    msg.gasRefund = this._runState.gasRefund

    // empty the return data buffer
    this._runState.returnBuffer = Buffer.alloc(0)

    // Check if account has enough ether and max depth not exceeded
    if (
      this._env.depth >= Number(this._common.param('vm', 'stackLimit')) ||
      (msg.delegatecall !== true && this._env.contract.balance < msg.value)
    ) {
      return BigInt(0)
    }

    const results = await this._evm.runCall({ message: msg })

    if (results.execResult.logs) {
      this._result.logs = this._result.logs.concat(results.execResult.logs)
    }

    // this should always be safe
    this.useGas(results.execResult.executionGasUsed, 'CALL, STATICCALL, DELEGATECALL, CALLCODE')

    // Set return value
    if (
      results.execResult.returnValue !== undefined &&
      (!results.execResult.exceptionError ||
        results.execResult.exceptionError.error === ERROR.REVERT)
    ) {
      this._runState.returnBuffer = results.execResult.returnValue
    }

    if (!results.execResult.exceptionError) {
      Object.assign(this._result.selfdestruct, selfdestruct)
      // update stateRoot on current contract
      const account = await this._eei.getAccount(this._env.address)
      this._env.contract = account
      this._runState.gasRefund = results.execResult.gasRefund ?? BigInt(0)
    }

    return this._getReturnCode(results)
  }

  /**
   * Creates a new contract with a given value.
   */
  async create(gasLimit: bigint, value: bigint, data: Buffer, salt?: Buffer): Promise<bigint> {
    const selfdestruct = { ...this._result.selfdestruct }
    const caller = this._env.address
    const depth = this._env.depth + 1

    // empty the return data buffer
    this._runState.returnBuffer = Buffer.alloc(0)

    // Check if account has enough ether and max depth not exceeded
    if (
      this._env.depth >= Number(this._common.param('vm', 'stackLimit')) ||
      this._env.contract.balance < value
    ) {
      return BigInt(0)
    }

    // EIP-2681 check
    if (this._env.contract.nonce >= MAX_UINT64) {
      return BigInt(0)
    }

    this._env.contract.nonce += BigInt(1)
    await this._eei.putAccount(this._env.address, this._env.contract)

    if (this._common.isActivatedEIP(3860)) {
      if (data.length > Number(this._common.param('vm', 'maxInitCodeSize'))) {
        return BigInt(0)
      }
    }

    const message = new Message({
      caller,
      gasLimit,
      value,
      data,
      salt,
      depth,
      selfdestruct,
      gasRefund: this._runState.gasRefund,
    })

    const results = await this._evm.runCall({ message })

    if (results.execResult.logs) {
      this._result.logs = this._result.logs.concat(results.execResult.logs)
    }

    // this should always be safe
    this.useGas(results.execResult.executionGasUsed, 'CREATE')

    // Set return buffer in case revert happened
    if (
      results.execResult.exceptionError &&
      results.execResult.exceptionError.error === ERROR.REVERT
    ) {
      this._runState.returnBuffer = results.execResult.returnValue
    }

    if (
      !results.execResult.exceptionError ||
      results.execResult.exceptionError.error === ERROR.CODESTORE_OUT_OF_GAS
    ) {
      Object.assign(this._result.selfdestruct, selfdestruct)
      // update stateRoot on current contract
      const account = await this._eei.getAccount(this._env.address)
      this._env.contract = account
      this._runState.gasRefund = results.execResult.gasRefund ?? BigInt(0)
      if (results.createdAddress) {
        // push the created address to the stack
        return bufferToBigInt(results.createdAddress.buf)
      }
    }

    return this._getReturnCode(results)
  }

  /**
   * Creates a new contract with a given value. Generates
   * a deterministic address via CREATE2 rules.
   */
  async create2(gasLimit: bigint, value: bigint, data: Buffer, salt: Buffer): Promise<bigint> {
    return this.create(gasLimit, value, data, salt)
  }

  /**
   * Mark account for later deletion and give the remaining balance to the
   * specified beneficiary address. This will cause a trap and the
   * execution will be aborted immediately.
   * @param toAddress - Beneficiary address
   */
  async selfDestruct(toAddress: Address): Promise<void> {
    return this._selfDestruct(toAddress)
  }

  async _selfDestruct(toAddress: Address): Promise<void> {
    // only add to refund if this is the first selfdestruct for the address
    if (this._result.selfdestruct[this._env.address.buf.toString('hex')] === undefined) {
      this.refundGas(this._common.param('gasPrices', 'selfdestructRefund'))
    }

    this._result.selfdestruct[this._env.address.buf.toString('hex')] = toAddress.buf

    // Add to beneficiary balance
    const toAccount = await this._eei.getAccount(toAddress)
    toAccount.balance += this._env.contract.balance
    await this._eei.putAccount(toAddress, toAccount)

    // Subtract from contract balance
    await this._eei.modifyAccountFields(this._env.address, {
      balance: BigInt(0),
    })

    trap(ERROR.STOP)
  }

  /**
   * Creates a new log in the current environment.
   */
  log(data: Buffer, numberOfTopics: number, topics: Buffer[]): void {
    if (numberOfTopics < 0 || numberOfTopics > 4) {
      trap(ERROR.OUT_OF_RANGE)
    }

    if (topics.length !== numberOfTopics) {
      trap(ERROR.INTERNAL_ERROR)
    }

    const log: Log = [this._env.address.buf, topics, data]
    this._result.logs.push(log)
  }

  private _getReturnCode(results: EVMResult) {
    // This preserves the previous logic, but seems to contradict the EEI spec
    // https://github.com/ewasm/design/blob/38eeded28765f3e193e12881ea72a6ab807a3371/eth_interface.md
    if (results.execResult.exceptionError) {
      return BigInt(0)
    } else {
      return BigInt(1)
    }
  }
}
