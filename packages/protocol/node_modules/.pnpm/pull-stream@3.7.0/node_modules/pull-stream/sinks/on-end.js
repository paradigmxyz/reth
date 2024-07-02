'use strict'

var drain = require('./drain')

module.exports = function onEnd (done) {
  return drain(null, done)
}
