const ethUtil = require('ethereumjs-util')
const MR = require('miller-rabin')
const BN = ethUtil.BN

exports.params = {
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
  WORD_BYTES: 4
}

exports.getCacheSize = function (epoc) {
  var mr = new MR()
  var sz =
    exports.params.CACHE_BYTES_INIT + exports.params.CACHE_BYTES_GROWTH * epoc
  sz -= exports.params.HASH_BYTES
  while (!mr.test(new BN(sz / exports.params.HASH_BYTES))) {
    sz -= 2 * exports.params.HASH_BYTES
  }
  return sz
}

exports.getFullSize = function (epoc) {
  var mr = new MR()
  var sz =
    exports.params.DATASET_BYTES_INIT +
    exports.params.DATASET_BYTES_GROWTH * epoc
  sz -= exports.params.MIX_BYTES
  while (!mr.test(new BN(sz / exports.params.MIX_BYTES))) {
    sz -= 2 * exports.params.MIX_BYTES
  }
  return sz
}

exports.getEpoc = function (blockNumber) {
  return Math.floor(blockNumber / exports.params.EPOCH_LENGTH)
}

/**
 * Generates a seed give the end epoc and optional the begining epoc and the
 * begining epoc seed
 * @method getSeed
 * @param end Number
 * @param begin Number
 * @param seed Buffer
 */
exports.getSeed = function (seed, begin, end) {
  for (var i = begin; i < end; i++) {
    seed = ethUtil.keccak256(seed)
  }
  return seed
}

var fnv = (exports.fnv = function (x, y) {
  return ((((x * 0x01000000) | 0) + ((x * 0x193) | 0)) ^ y) >>> 0
})

exports.fnvBuffer = function (a, b) {
  var r = Buffer.alloc(a.length)
  for (var i = 0; i < a.length; i = i + 4) {
    r.writeUInt32LE(fnv(a.readUInt32LE(i), b.readUInt32LE(i)), i)
  }
  return r
}

exports.bufReverse = function (a) {
  const length = a.length
  var b = Buffer.alloc(length)
  for (var i = 0; i < length; i++) {
    b[i] = a[length - i - 1]
  }
  return b
}
