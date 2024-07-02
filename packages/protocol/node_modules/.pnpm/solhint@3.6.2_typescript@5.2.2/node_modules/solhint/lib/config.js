const _ = require('lodash')

module.exports = {
  from(configVals) {
    return _.assign({ rules: {} }, configVals, this)
  },

  getNumberByPath(path, defaultValue) {
    const configVal = _.get(this, path)
    return _.isNumber(configVal) ? configVal : defaultValue
  },

  getBooleanByPath(path, defaultValue) {
    const configVal = _.get(this, path)
    return _.isBoolean(configVal) ? configVal : defaultValue
  },

  getNumber(ruleName, defaultValue) {
    return this.getNumberByPath(`rules["${ruleName}"][1]`, defaultValue)
  },

  getObjectPropertyNumber(ruleName, ruleProperty, defaultValue) {
    return this.getNumberByPath(`rules["${ruleName}"][1][${ruleProperty}]`, defaultValue)
  },

  getObjectPropertyBoolean(ruleName, ruleProperty, defaultValue) {
    return this.getBooleanByPath(`rules["${ruleName}"][1][${ruleProperty}]`, defaultValue)
  },

  getStringFromArray(ruleName, defaultValue) {
    if (this.rules && this.rules[ruleName]) {
      const ruleValue = this.rules[ruleName]
      return Array.isArray(ruleValue) ? ruleValue[1] : defaultValue
    } else {
      return defaultValue
    }
  },

  getArray(ruleName, defaultValue) {
    const path = `rules["${ruleName}"][1]`
    const configVal = _.get(this, path)
    return _.isArray(configVal) ? configVal : defaultValue
  },
}
