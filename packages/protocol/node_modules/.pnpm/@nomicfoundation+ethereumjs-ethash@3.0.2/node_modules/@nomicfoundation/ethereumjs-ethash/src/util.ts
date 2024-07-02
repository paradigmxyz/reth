import { isProbablyPrime } from 'bigint-crypto-utils'
import { keccak256 } from 'ethereum-cryptography/keccak'

export const params = {
  DATASET_BYTES_INIT: 1073741824, // 2^30
  DATASET_BYTES_GROWTH: 8388608, // 2 ^ 23
  CACHE_BYTES_INIT: 16777216, // 2**24 number of bytes in dataset at genesis
  CACHE_BYTES_GROWTH: 131072, // 2**17 cache growth per epoch
  CACHE_MULTIPLIER: 1024, // Size of the DAG relative to the cache
  EPOCH_LENGTH: 30000, // blocks per epoch
  MIX_BYTES: 128, // width of mix
  HASH_BYTES: 64, // hash length in bytes
  DATASET_PARENTS: 256, // number of parents of each dataset element
  CACHE_ROUNDS: 3, // number of rounds in cache production
  ACCESSES: 64,
  WORD_BYTES: 4,
}

export async function getCacheSize(epoc: number) {
  const { CACHE_BYTES_INIT, CACHE_BYTES_GROWTH, HASH_BYTES } = params
  let sz = CACHE_BYTES_INIT + CACHE_BYTES_GROWTH * epoc
  sz -= HASH_BYTES
  while (!(await isProbablyPrime(sz / HASH_BYTES, undefined, true))) {
    sz -= 2 * HASH_BYTES
  }
  return sz
}

export async function getFullSize(epoc: number) {
  const { DATASET_BYTES_INIT, DATASET_BYTES_GROWTH, MIX_BYTES } = params
  let sz = DATASET_BYTES_INIT + DATASET_BYTES_GROWTH * epoc
  sz -= MIX_BYTES
  while (!(await isProbablyPrime(sz / MIX_BYTES, undefined, true))) {
    sz -= 2 * MIX_BYTES
  }
  return sz
}

export function getEpoc(blockNumber: bigint) {
  return Number(blockNumber / BigInt(params.EPOCH_LENGTH))
}

/**
 * Generates a seed give the end epoc and optional the beginning epoc and the
 * beginning epoc seed
 * @method getSeed
 * @param seed Buffer
 * @param begin Number
 * @param end Number
 */
export function getSeed(seed: Buffer, begin: number, end: number) {
  for (let i = begin; i < end; i++) {
    seed = Buffer.from(keccak256(seed))
  }
  return seed
}

export function fnv(x: number, y: number) {
  return ((((x * 0x01000000) | 0) + ((x * 0x193) | 0)) ^ y) >>> 0
}

export function fnvBuffer(a: Buffer, b: Buffer) {
  const r = Buffer.alloc(a.length)
  for (let i = 0; i < a.length; i = i + 4) {
    r.writeUInt32LE(fnv(a.readUInt32LE(i), b.readUInt32LE(i)), i)
  }
  return r
}

export function bufReverse(a: Buffer) {
  const length = a.length
  const b = Buffer.alloc(length)
  for (let i = 0; i < length; i++) {
    b[i] = a[length - i - 1]
  }
  return b
}
