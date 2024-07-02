'use strict'
//a stream that errors immediately.
module.exports = function error (err) {
  return function (abort, cb) {
    cb(err)
  }
}

