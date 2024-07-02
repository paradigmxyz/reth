'use strict'
const arrayify = require('array-back')
const option = require('./option')

/**
 * Handles parsing different argv notations
 *
 * @module argv
 * @private
 */

class Argv extends Array {
  load (argv) {
    if (argv) {
      argv = arrayify(argv)
    } else {
      /* if no argv supplied, assume we are parsing process.argv */
      argv = process.argv.slice(0)
      argv.splice(0, 2)
    }
    argv.forEach(arg => this.push(String(arg)))
  }

  clear () {
    this.length = 0
  }

  /**
   * expand --option=value style args. The value is clearly marked to indicate it is definitely a value (which would otherwise be unclear if the value is `--value`, which would be parsed as an option). The special marker is removed in parsing phase.
   */
  expandOptionEqualsNotation () {
    const optEquals = option.optEquals
    if (this.some(optEquals.test.bind(optEquals))) {
      const expandedArgs = []
      this.forEach(arg => {
        const matches = arg.match(optEquals)
        if (matches) {
          expandedArgs.push(matches[1], option.VALUE_MARKER + matches[2])
        } else {
          expandedArgs.push(arg)
        }
      })
      this.clear()
      this.load(expandedArgs)
    }
  }

  /**
   * expand getopt-style combined options
   */
  expandGetoptNotation () {
    const findReplace = require('find-replace')
    const combinedArg = option.combined
    const hasGetopt = this.some(combinedArg.test.bind(combinedArg))
    if (hasGetopt) {
      findReplace(this, combinedArg, arg => {
        arg = arg.slice(1)
        return arg.split('').map(letter => '-' + letter)
      })
    }
  }

  /**
   * Inspect the user-supplied options for validation issues.
   * @throws `UNKNOWN_OPTION`
   */
  validate (definitions, options) {
    options = options || {}
    let invalidOption

    if (!options.partial) {
      const optionWithoutDefinition = this
        .filter(arg => option.isOption(arg))
        .some(arg => {
          if (definitions.get(arg) === undefined) {
            invalidOption = arg
            return true
          }
        })
      if (optionWithoutDefinition) {
        halt(
          'UNKNOWN_OPTION',
          'Unknown option: ' + invalidOption
        )
      }
    }
  }
}

function halt (name, message) {
  const err = new Error(message)
  err.name = name
  throw err
}

module.exports = Argv
