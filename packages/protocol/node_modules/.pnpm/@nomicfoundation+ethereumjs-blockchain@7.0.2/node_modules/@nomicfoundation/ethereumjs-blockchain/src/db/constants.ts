import { bigIntToBuffer } from '@nomicfoundation/ethereumjs-util'

// Geth compatible DB keys

const HEADS_KEY = 'heads'

/**
 * Current canonical head for light sync
 */
const HEAD_HEADER_KEY = 'LastHeader'

/**
 * Current canonical head for full sync
 */
const HEAD_BLOCK_KEY = 'LastBlock'

/**
 * headerPrefix + number + hash -> header
 */
const HEADER_PREFIX = Buffer.from('h')

/**
 * headerPrefix + number + hash + tdSuffix -> td
 */
const TD_SUFFIX = Buffer.from('t')

/**
 * headerPrefix + number + numSuffix -> hash
 */
const NUM_SUFFIX = Buffer.from('n')

/**
 * blockHashPrefix + hash -> number
 */
const BLOCK_HASH_PEFIX = Buffer.from('H')

/**
 * bodyPrefix + number + hash -> block body
 */
const BODY_PREFIX = Buffer.from('b')

// Utility functions

/**
 * Convert bigint to big endian Buffer
 */
const bufBE8 = (n: bigint) => bigIntToBuffer(BigInt.asUintN(64, n))

const tdKey = (n: bigint, hash: Buffer) =>
  Buffer.concat([HEADER_PREFIX, bufBE8(n), hash, TD_SUFFIX])

const headerKey = (n: bigint, hash: Buffer) => Buffer.concat([HEADER_PREFIX, bufBE8(n), hash])

const bodyKey = (n: bigint, hash: Buffer) => Buffer.concat([BODY_PREFIX, bufBE8(n), hash])

const numberToHashKey = (n: bigint) => Buffer.concat([HEADER_PREFIX, bufBE8(n), NUM_SUFFIX])

const hashToNumberKey = (hash: Buffer) => Buffer.concat([BLOCK_HASH_PEFIX, hash])

/**
 * @hidden
 */
export {
  bodyKey,
  bufBE8,
  hashToNumberKey,
  HEAD_BLOCK_KEY,
  HEAD_HEADER_KEY,
  headerKey,
  HEADS_KEY,
  numberToHashKey,
  tdKey,
}
