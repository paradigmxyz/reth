'use strict'

const ModuleError = require('module-error')
const hasOwnProperty = Object.prototype.hasOwnProperty
const rangeOptions = new Set(['lt', 'lte', 'gt', 'gte'])

module.exports = function (options, keyEncoding) {
  const result = {}

  for (const k in options) {
    if (!hasOwnProperty.call(options, k)) continue
    if (k === 'keyEncoding' || k === 'valueEncoding') continue

    if (k === 'start' || k === 'end') {
      throw new ModuleError(`The legacy range option '${k}' has been removed`, {
        code: 'LEVEL_LEGACY'
      })
    } else if (k === 'encoding') {
      // To help migrating to abstract-level
      throw new ModuleError("The levelup-style 'encoding' alias has been removed, use 'valueEncoding' instead", {
        code: 'LEVEL_LEGACY'
      })
    }

    if (rangeOptions.has(k)) {
      // Note that we don't reject nullish and empty options here. While
      // those types are invalid as keys, they are valid as range options.
      result[k] = keyEncoding.encode(options[k])
    } else {
      result[k] = options[k]
    }
  }

  result.reverse = !!result.reverse
  result.limit = Number.isInteger(result.limit) && result.limit >= 0 ? result.limit : -1

  return result
}
