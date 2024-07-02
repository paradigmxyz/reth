/* global IDBKeyRange */

'use strict'

module.exports = function createKeyRange (options) {
  const lower = options.gte !== undefined ? options.gte : options.gt !== undefined ? options.gt : undefined
  const upper = options.lte !== undefined ? options.lte : options.lt !== undefined ? options.lt : undefined
  const lowerExclusive = options.gte === undefined
  const upperExclusive = options.lte === undefined

  if (lower !== undefined && upper !== undefined) {
    return IDBKeyRange.bound(lower, upper, lowerExclusive, upperExclusive)
  } else if (lower !== undefined) {
    return IDBKeyRange.lowerBound(lower, lowerExclusive)
  } else if (upper !== undefined) {
    return IDBKeyRange.upperBound(upper, upperExclusive)
  } else {
    return null
  }
}
