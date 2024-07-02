import { Address } from '@nomicfoundation/ethereumjs-util'

import type { PrecompileFunc } from './precompiles'

const defaults = {
  value: BigInt(0),
  caller: Address.zero(),
  data: Buffer.alloc(0),
  depth: 0,
  isStatic: false,
  isCompiled: false,
  delegatecall: false,
  gasRefund: BigInt(0),
}

interface MessageOpts {
  to?: Address
  value?: bigint
  caller?: Address
  gasLimit: bigint
  data?: Buffer
  depth?: number
  code?: Buffer | PrecompileFunc
  codeAddress?: Address
  isStatic?: boolean
  isCompiled?: boolean
  salt?: Buffer
  /**
   * A map of addresses to selfdestruct, see {@link Message.selfdestruct}
   */
  selfdestruct?: { [key: string]: boolean } | { [key: string]: Buffer }
  delegatecall?: boolean
  authcallOrigin?: Address
  gasRefund?: bigint
  versionedHashes?: Buffer[]
}

export class Message {
  to?: Address
  value: bigint
  caller: Address
  gasLimit: bigint
  data: Buffer
  depth: number
  code?: Buffer | PrecompileFunc
  _codeAddress?: Address
  isStatic: boolean
  isCompiled: boolean
  salt?: Buffer
  containerCode?: Buffer /** container code for EOF1 contracts - used by CODECOPY/CODESIZE */
  /**
   * Map of addresses to selfdestruct. Key is the unprefixed address.
   * Value is a boolean when marked for destruction and replaced with a Buffer containing the address where the remaining funds are sent.
   */
  selfdestruct?: { [key: string]: boolean } | { [key: string]: Buffer }
  delegatecall: boolean
  /**
   * This is used to store the origin of the AUTHCALL,
   * the purpose is to figure out where `value` should be taken from (not from `caller`)
   */
  authcallOrigin?: Address
  gasRefund: bigint // Keeps track of the gasRefund at the start of the frame (used for journaling purposes)
  /**
   * List of versioned hashes if message is a blob transaction in the outer VM
   */
  versionedHashes?: Buffer[]

  constructor(opts: MessageOpts) {
    this.to = opts.to
    this.value = opts.value ?? defaults.value
    this.caller = opts.caller ?? defaults.caller
    this.gasLimit = opts.gasLimit
    this.data = opts.data ?? defaults.data
    this.depth = opts.depth ?? defaults.depth
    this.code = opts.code
    this._codeAddress = opts.codeAddress
    this.isStatic = opts.isStatic ?? defaults.isStatic
    this.isCompiled = opts.isCompiled ?? defaults.isCompiled
    this.salt = opts.salt
    this.selfdestruct = opts.selfdestruct
    this.delegatecall = opts.delegatecall ?? defaults.delegatecall
    this.authcallOrigin = opts.authcallOrigin
    this.gasRefund = opts.gasRefund ?? defaults.gasRefund
    this.versionedHashes = opts.versionedHashes
    if (this.value < 0) {
      throw new Error(`value field cannot be negative, received ${this.value}`)
    }
  }

  /**
   * Note: should only be called in instances where `_codeAddress` or `to` is defined.
   */
  get codeAddress(): Address {
    const codeAddress = this._codeAddress ?? this.to
    if (!codeAddress) {
      throw new Error('Missing codeAddress')
    }
    return codeAddress
  }
}

export type MessageWithTo = Message & Pick<Required<MessageOpts>, 'to'>
