const BaseChecker = require('../base-checker')
const naming = require('../../common/identifier-naming')

const ruleId = 'func-name-mixedcase'
const meta = {
  type: 'naming',

  docs: {
    description: 'Function name must be in mixedCase.',
    category: 'Style Guide Rules',
  },

  isDefault: false,
  recommended: true,
  defaultSetup: 'warn',

  schema: null,
}

class FuncNameMixedcaseChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  FunctionDefinition(node) {
    if (naming.isNotMixedCase(node.name) && !node.isConstructor) {
      this.error(node, 'Function name must be in mixedCase')
    }
  }
}

module.exports = FuncNameMixedcaseChecker
