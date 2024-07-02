'use strict'
const arrayify = require('array-back')
const Output = require('./output')

class GroupedOutput extends Output {
  toObject () {
    const superOutput = super.toObject()
    delete superOutput._unknown
    const grouped = {
      _all: superOutput
    }
    if (this.unknown.length) grouped._unknown = this.unknown

    this.definitions.whereGrouped().forEach(def => {
      const outputValue = this.output[def.name]
      for (const groupName of arrayify(def.group)) {
        grouped[groupName] = grouped[groupName] || {}
        if (outputValue && outputValue.isDefined()) {
          grouped[groupName][def.name] = outputValue.value
        }
      }
    })

    this.definitions.whereNotGrouped().forEach(def => {
      const outputValue = this.output[def.name]
      if (outputValue && outputValue.isDefined()) {
        if (!grouped._none) grouped._none = {}
        grouped._none[def.name] = outputValue.value
      }
    })
    return grouped
  }
}

module.exports = GroupedOutput
