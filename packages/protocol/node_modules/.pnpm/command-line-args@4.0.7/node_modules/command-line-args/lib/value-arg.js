'use strict'
const t = require('typical')
const option = require('./option')
const reBeginsWithValueMarker = new RegExp('^' + option.VALUE_MARKER)

class ValueArg {
  constructor (value) {
    this.isOptionValueNotationValue = reBeginsWithValueMarker.test(value)
    /* if the value marker is present at the value beginning, strip it */
    this.value = value ? value.replace(reBeginsWithValueMarker, '') : value
  }

  isDefined () {
    return t.isDefined(this.value)
  }
}

module.exports = ValueArg
