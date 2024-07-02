'use strict'
var values = require('./values')
module.exports = function (object) {
  return values(Object.keys(object))
}


