'use strict'

var reduce = require('./reduce')

module.exports = function collect (cb) {
  return reduce(function (arr, item) {
    arr.push(item)
    return arr
  }, [], cb)
}
