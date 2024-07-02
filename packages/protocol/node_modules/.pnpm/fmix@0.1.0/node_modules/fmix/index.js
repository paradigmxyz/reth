var imul = require('imul')

module.exports = function fmix (h) {
  h ^= (h >>> 16)
  h = imul(h, 0x85ebca6b)
  h ^= (h >>> 13)
  h = imul(h, 0xc2b2ae35)
  h ^= (h >>> 16)

  return (h >>> 0)
}
