var imul = require('imul')
var fmix = require('fmix')
var encodeUtf8 = require('encode-utf8')

var C = new Uint32Array([
  0x239b961b,
  0xab0e9789,
  0x38b34ae5,
  0xa1e38b93
])

function rotl (m, n) {
  return (m << n) | (m >>> (32 - n))
}

function body (key, hash) {
  var blocks = (key.byteLength / 16) | 0
  var view32 = new Uint32Array(key, 0, blocks * 4)

  var k
  for (var i = 0; i < blocks; i++) {
    k = view32.subarray(i * 4, (i + 1) * 4)

    k[0] = imul(k[0], C[0])
    k[0] = rotl(k[0], 15)
    k[0] = imul(k[0], C[1])

    hash[0] = (hash[0] ^ k[0])
    hash[0] = rotl(hash[0], 19)
    hash[0] = (hash[0] + hash[1])
    hash[0] = imul(hash[0], 5) + 0x561ccd1b

    k[1] = imul(k[1], C[1])
    k[1] = rotl(k[1], 16)
    k[1] = imul(k[1], C[2])

    hash[1] = (hash[1] ^ k[1])
    hash[1] = rotl(hash[1], 17)
    hash[1] = (hash[1] + hash[2])
    hash[1] = imul(hash[1], 5) + 0x0bcaa747

    k[2] = imul(k[2], C[2])
    k[2] = rotl(k[2], 17)
    k[2] = imul(k[2], C[3])

    hash[2] = (hash[2] ^ k[2])
    hash[2] = rotl(hash[2], 15)
    hash[2] = (hash[2] + hash[3])
    hash[2] = imul(hash[2], 5) + 0x96cd1c35

    k[3] = imul(k[3], C[3])
    k[3] = rotl(k[3], 18)
    k[3] = imul(k[3], C[0])

    hash[3] = (hash[3] ^ k[3])
    hash[3] = rotl(hash[3], 13)
    hash[3] = (hash[3] + hash[0])
    hash[3] = imul(hash[3], 5) + 0x32ac3b17
  }
}

function tail (key, hash) {
  var blocks = (key.byteLength / 16) | 0
  var reminder = (key.byteLength % 16)

  var k = new Uint32Array(4)
  var tail = new Uint8Array(key, blocks * 16, reminder)

  switch (reminder) {
    case 15:
      k[3] = (k[3] ^ (tail[14] << 16))
      // fallthrough
    case 14:
      k[3] = (k[3] ^ (tail[13] << 8))
      // fallthrough
    case 13:
      k[3] = (k[3] ^ (tail[12] << 0))

      k[3] = imul(k[3], C[3])
      k[3] = rotl(k[3], 18)
      k[3] = imul(k[3], C[0])
      hash[3] = (hash[3] ^ k[3])
      // fallthrough
    case 12:
      k[2] = (k[2] ^ (tail[11] << 24))
      // fallthrough
    case 11:
      k[2] = (k[2] ^ (tail[10] << 16))
      // fallthrough
    case 10:
      k[2] = (k[2] ^ (tail[9] << 8))
      // fallthrough
    case 9:
      k[2] = (k[2] ^ (tail[8] << 0))

      k[2] = imul(k[2], C[2])
      k[2] = rotl(k[2], 17)
      k[2] = imul(k[2], C[3])
      hash[2] = (hash[2] ^ k[2])
      // fallthrough
    case 8:
      k[1] = (k[1] ^ (tail[7] << 24))
      // fallthrough
    case 7:
      k[1] = (k[1] ^ (tail[6] << 16))
      // fallthrough
    case 6:
      k[1] = (k[1] ^ (tail[5] << 8))
      // fallthrough
    case 5:
      k[1] = (k[1] ^ (tail[4] << 0))

      k[1] = imul(k[1], C[1])
      k[1] = rotl(k[1], 16)
      k[1] = imul(k[1], C[2])
      hash[1] = (hash[1] ^ k[1])
      // fallthrough
    case 4:
      k[0] = (k[0] ^ (tail[3] << 24))
      // fallthrough
    case 3:
      k[0] = (k[0] ^ (tail[2] << 16))
      // fallthrough
    case 2:
      k[0] = (k[0] ^ (tail[1] << 8))
      // fallthrough
    case 1:
      k[0] = (k[0] ^ (tail[0] << 0))

      k[0] = imul(k[0], C[0])
      k[0] = rotl(k[0], 15)
      k[0] = imul(k[0], C[1])
      hash[0] = (hash[0] ^ k[0])
  }
}

function finalize (key, hash) {
  hash[0] = (hash[0] ^ key.byteLength)
  hash[1] = (hash[1] ^ key.byteLength)
  hash[2] = (hash[2] ^ key.byteLength)
  hash[3] = (hash[3] ^ key.byteLength)

  hash[0] = (hash[0] + hash[1]) | 0
  hash[0] = (hash[0] + hash[2]) | 0
  hash[0] = (hash[0] + hash[3]) | 0

  hash[1] = (hash[1] + hash[0]) | 0
  hash[2] = (hash[2] + hash[0]) | 0
  hash[3] = (hash[3] + hash[0]) | 0

  hash[0] = fmix(hash[0])
  hash[1] = fmix(hash[1])
  hash[2] = fmix(hash[2])
  hash[3] = fmix(hash[3])

  hash[0] = (hash[0] + hash[1]) | 0
  hash[0] = (hash[0] + hash[2]) | 0
  hash[0] = (hash[0] + hash[3]) | 0

  hash[1] = (hash[1] + hash[0]) | 0
  hash[2] = (hash[2] + hash[0]) | 0
  hash[3] = (hash[3] + hash[0]) | 0
}

module.exports = function murmur (key, seed) {
  seed = (seed ? (seed | 0) : 0)

  if (typeof key === 'string') {
    key = encodeUtf8(key)
  }

  if (!(key instanceof ArrayBuffer)) {
    throw new TypeError('Expected key to be ArrayBuffer or string')
  }

  var hash = new Uint32Array([seed, seed, seed, seed])

  body(key, hash)
  tail(key, hash)
  finalize(key, hash)

  return hash.buffer
}
