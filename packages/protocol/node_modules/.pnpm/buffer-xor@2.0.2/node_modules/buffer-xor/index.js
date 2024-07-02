var Buffer = require('safe-buffer').Buffer

module.exports = function xor (a, b) {
  var length = Math.max(a.length, b.length)
  var buffer = Buffer.allocUnsafe(length)

  for (var i = 0; i < length; ++i) {
    buffer[i] = a[i] ^ b[i]
  }

  return buffer
}
