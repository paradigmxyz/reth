'use strict'
var t = require('typical')

/**
 * @module array-back
 * @example
 * var arrayify = require("array-back")
 */
module.exports = arrayify

/**
 * Takes any input and guarantees an array back.
 *
 * - converts array-like objects (e.g. `arguments`) to a real array
 * - converts `undefined` to an empty array
 * - converts any another other, singular value (including `null`) into an array containing that value
 * - ignores input which is already an array
 *
 * @param {*} - the input value to convert to an array
 * @returns {Array}
 * @alias module:array-back
 * @example
 * > a.arrayify(undefined)
 * []
 *
 * > a.arrayify(null)
 * [ null ]
 *
 * > a.arrayify(0)
 * [ 0 ]
 *
 * > a.arrayify([ 1, 2 ])
 * [ 1, 2 ]
 *
 * > function f(){ return a.arrayify(arguments); }
 * > f(1,2,3)
 * [ 1, 2, 3 ]
 */
function arrayify (input) {
  if (input === undefined) {
    return []
  } else if (t.isArrayLike(input)) {
    return Array.prototype.slice.call(input)
  } else {
    return Array.isArray(input) ? input : [ input ]
  }
}
