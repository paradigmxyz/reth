const BaseChecker = require('../base-checker')
const naming = require('../../common/identifier-naming')

const ruleId = 'func-param-name-mixedcase'
const meta = {
  type: 'naming',

  docs: {
    description: 'Function param name must be in mixedCase.',
    category: 'Style Guide Rules',
  },

  isDefault: false,
  recommended: false,
  defaultSetup: 'warn',

  schema: null,
}

class FunctionParamNameMixedcaseChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  EventDefinition(node) {
    this.FunctionDefinition(node)
  }

  FunctionDefinition(node) {
    node.parameters.forEach((parameter) => {
      if (naming.isNotMixedCase(parameter.name)) {
        this.error(parameter, 'Function param name must be in mixedCase')
      }
    })
  }
}

module.exports = FunctionParamNameMixedcaseChecker
