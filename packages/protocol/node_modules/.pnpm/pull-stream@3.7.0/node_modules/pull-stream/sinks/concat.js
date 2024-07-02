'use strict'

var reduce = require('./reduce')

module.exports = function concat (cb) {
  return reduce(function (a, b) {
    return a + b
  }, '', cb)
}
