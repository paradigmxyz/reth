'use strict'

var drain = require('./drain')

module.exports = function log (done) {
  return drain(function (data) {
    console.log(data)
  }, done)
}
