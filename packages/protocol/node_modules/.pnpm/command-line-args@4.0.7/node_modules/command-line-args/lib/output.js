'use strict'
const t = require('typical')
const arrayify = require('array-back')

class OutputValue {
  constructor (value) {
    this.value = value
    this.hasDefaultArrayValue = false
    this.valueSource = 'unknown'
  }

  isDefined () {
    return t.isDefined(this.value)
  }
}

class Output {
  constructor (definitions, options) {
    this.options = options || {}
    this.output = {}
    this.unknown = []
    this.definitions = definitions
    this._assignDefaultValues()
  }

  _assignDefaultValues () {
    this.definitions.forEach(def => {
      if (t.isDefined(def.defaultValue)) {
        if (def.multiple) {
          this.output[def.name] = new OutputValue(arrayify(def.defaultValue))
          this.output[def.name].hasDefaultArrayValue = true
        } else {
          this.output[def.name] = new OutputValue(def.defaultValue)
        }
        this.output[def.name].valueSource = 'default'
      }
    })
  }

  setFlag (optionArg) {
    const def = this.definitions.get(optionArg)

    if (def) {
      this.output[def.name] = this.output[def.name] || new OutputValue()
      const outputValue = this.output[def.name]

      if (def.multiple) outputValue.value = outputValue.value || []

      /* for boolean types, set value to `true`. For all other types run value through setter function. */
      if (def.isBoolean()) {
        if (Array.isArray(outputValue.value)) {
          outputValue.value.push(true)
        } else {
          outputValue.value = true
        }
        return true
      } else {
        if (!Array.isArray(outputValue.value) && outputValue.valueSource === 'unknown') outputValue.value = null
        return false
      }
    } else {
      this.unknown.push(optionArg)
      return true
    }
  }

  setOptionValue (optionArg, value) {
    const ValueArg = require('./value-arg')
    const valueArg = new ValueArg(value)

    const def = this.definitions.get(optionArg)

    this.output[def.name] = this.output[def.name] || new OutputValue()
    const outputValue = this.output[def.name]

    if (def.multiple) outputValue.value = outputValue.value || []

    /* run value through setter function. */
    valueArg.value = def.type(valueArg.value)
    outputValue.valueSource = 'argv'
    if (Array.isArray(outputValue.value)) {
      if (outputValue.hasDefaultArrayValue) {
        outputValue.value = [ valueArg.value ]
        outputValue.hasDefaultArrayValue = false
      } else {
        outputValue.value.push(valueArg.value)
      }
      return false
    } else {
      outputValue.value = valueArg.value
      return true
    }
  }

  /**
   * Return `true` when an option value was set and is not a multiple. Return `false` if option was a multiple or if a value was not yet set.
   */
  setValue (value) {
    const ValueArg = require('./value-arg')
    const valueArg = new ValueArg(value)

    /* use the defaultOption */
    const def = this.definitions.getDefault()

    /* handle unknown values in the case a value was already set on a defaultOption */
    if (def) {
      const currentValue = this.output[def.name]
      if (valueArg.isDefined() && currentValue && t.isDefined(currentValue.value)) {
        if (def.multiple) {
          /* in the case we're setting an --option=value value on a multiple defaultOption, tag the value onto the previous unknown */
          if (valueArg.isOptionValueNotationValue && this.unknown.length) {
            this.unknown[this.unknown.length - 1] += `=${valueArg.value}`
            return true
          }
        } else {
          /* currentValue has already been set by argv,log this value as unknown and move on */
          if (currentValue.valueSource === 'argv') {
            this.unknown.push(valueArg.value)
            return true
          }
        }
      }
      return this.setOptionValue(`--${def.name}`, value)
    } else {
      if (valueArg.isOptionValueNotationValue) {
        this.unknown[this.unknown.length - 1] += `=${valueArg.value}`
      } else {
        this.unknown.push(valueArg.value)
      }
      return true
    }
  }

  get (name) {
    return this.output[name] && this.output[name].value
  }

  toObject () {
    let output = Object.assign({}, this.output)
    if (this.options.partial && this.unknown.length) {
      output._unknown = this.unknown
    }
    for (const prop in output) {
      if (prop !== '_unknown') {
        output[prop] = output[prop].value
      }
    }
    return output
  }
}

module.exports = Output
