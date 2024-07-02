'use strict'
//a stream that ends immediately.
module.exports = function empty () {
  return function (abort, cb) {
    cb(true)
  }
}
