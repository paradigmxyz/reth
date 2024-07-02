'use strict'
var arrayify = require('array-back')
var testValue = require('test-value')

/**
 * Find and either replace or remove items from an array.
 *
 * @module find-replace
 * @example
 * > findReplace = require('find-replace')
 *
 * > findReplace([ 1, 2, 3], 2, 'two')
 * [ 1, 'two', 3 ]
 *
 * > findReplace([ 1, 2, 3], 2, [ 'two', 'zwei' ])
 * [ 1, [ 'two', 'zwei' ], 3 ]
 *
 * > findReplace([ 1, 2, 3], 2, 'two', 'zwei')
 * [ 1, 'two', 'zwei', 3 ]
 *
 * > findReplace([ 1, 2, 3], 2) // no replacement, so remove
 * [ 1, 3 ]
 */
module.exports = findReplace

/**
 * @param {array} - the input array
 * @param {valueTest} - a [test-value](https://github.com/75lb/test-value) query to match the value you're looking for
 * @param [replaceWith] {...any} - If specified, found values will be replaced with these values, else  removed.
 * @returns {array}
 * @alias module:find-replace
 */
function findReplace (array, valueTest) {
  var found = []
  var replaceWiths = arrayify(arguments)
  replaceWiths.splice(0, 2)

  arrayify(array).forEach(function (value, index) {
    var expanded = []
    replaceWiths.forEach(function (replaceWith) {
      if (typeof replaceWith === 'function') {
        expanded = expanded.concat(replaceWith(value))
      } else {
        expanded.push(replaceWith)
      }
    })

    if (testValue(value, valueTest)) {
      found.push({
        index: index,
        replaceWithValue: expanded
      })
    }
  })

  found.reverse().forEach(function (item) {
    var spliceArgs = [ item.index, 1 ].concat(item.replaceWithValue)
    array.splice.apply(array, spliceArgs)
  })

  return array
}
