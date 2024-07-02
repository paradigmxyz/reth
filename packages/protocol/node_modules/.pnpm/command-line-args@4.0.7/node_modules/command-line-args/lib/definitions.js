'use strict'
const arrayify = require('array-back')
const option = require('./option')
const Definition = require('./definition')
const t = require('typical')

/**
 * @module definitions
 * @private
 */

/**
 * @alias module:definitions
 */
class Definitions extends Array {
  load (definitions) {
    this.clear()
    arrayify(definitions).forEach(def => this.push(new Definition(def)))
    this.validate()
  }

  clear () {
    this.length = 0
  }

  /**
   * validate option definitions
   * @returns {string}
   */
  validate (argv) {
    const someHaveNoName = this.some(def => !def.name)
    if (someHaveNoName) {
      halt(
        'NAME_MISSING',
        'Invalid option definitions: the `name` property is required on each definition'
      )
    }

    const someDontHaveFunctionType = this.some(def => def.type && typeof def.type !== 'function')
    if (someDontHaveFunctionType) {
      halt(
        'INVALID_TYPE',
        'Invalid option definitions: the `type` property must be a setter fuction (default: `Boolean`)'
      )
    }

    let invalidOption

    const numericAlias = this.some(def => {
      invalidOption = def
      return t.isDefined(def.alias) && t.isNumber(def.alias)
    })
    if (numericAlias) {
      halt(
        'INVALID_ALIAS',
        'Invalid option definition: to avoid ambiguity an alias cannot be numeric [--' + invalidOption.name + ' alias is -' + invalidOption.alias + ']'
      )
    }

    const multiCharacterAlias = this.some(def => {
      invalidOption = def
      return t.isDefined(def.alias) && def.alias.length !== 1
    })
    if (multiCharacterAlias) {
      halt(
        'INVALID_ALIAS',
        'Invalid option definition: an alias must be a single character'
      )
    }

    const hypenAlias = this.some(def => {
      invalidOption = def
      return def.alias === '-'
    })
    if (hypenAlias) {
      halt(
        'INVALID_ALIAS',
        'Invalid option definition: an alias cannot be "-"'
      )
    }

    const duplicateName = hasDuplicates(this.map(def => def.name))
    if (duplicateName) {
      halt(
        'DUPLICATE_NAME',
        'Two or more option definitions have the same name'
      )
    }

    const duplicateAlias = hasDuplicates(this.map(def => def.alias))
    if (duplicateAlias) {
      halt(
        'DUPLICATE_ALIAS',
        'Two or more option definitions have the same alias'
      )
    }

    const duplicateDefaultOption = hasDuplicates(this.map(def => def.defaultOption))
    if (duplicateDefaultOption) {
      halt(
        'DUPLICATE_DEFAULT_OPTION',
        'Only one option definition can be the defaultOption'
      )
    }
  }

  /**
   * @param {string}
   * @returns {Definition}
   */
  get (arg) {
    return option.short.test(arg)
      ? this.find(def => def.alias === option.short.name(arg))
      : this.find(def => def.name === option.long.name(arg))
  }

  getDefault () {
    return this.find(def => def.defaultOption === true)
  }

  isGrouped () {
    return this.some(def => def.group)
  }

  whereGrouped () {
    return this.filter(containsValidGroup)
  }
  whereNotGrouped () {
    return this.filter(def => !containsValidGroup(def))
  }
}

function halt (name, message) {
  const err = new Error(message)
  err.name = name
  throw err
}

function containsValidGroup (def) {
  return arrayify(def.group).some(group => group)
}

function hasDuplicates (array) {
  const items = {}
  for (let i = 0; i < array.length; i++) {
    const value = array[i]
    if (items[value]) {
      return true
    } else {
      if (t.isDefined(value)) items[value] = true
    }
  }
}

module.exports = Definitions
