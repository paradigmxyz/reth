const BaseChecker = require('../base-checker')
const naming = require('../../common/identifier-naming')

const ruleId = 'var-name-mixedcase'
const meta = {
  type: 'naming',

  docs: {
    description: `Variable name must be in mixedCase. (Does not check IMMUTABLES, use immutable-vars-naming)`,
    category: 'Style Guide Rules',
  },

  isDefault: false,
  recommended: true,
  defaultSetup: 'warn',

  schema: null,
}

class VarNameMixedcaseChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  VariableDeclaration(node) {
    if (!node.isDeclaredConst && !node.isImmutable) {
      this.validateVariablesName(node)
    }
  }

  validateVariablesName(node) {
    if (naming.isNotMixedCase(node.name)) {
      this.error(node, 'Variable name must be in mixedCase')
    }
  }
}

module.exports = VarNameMixedcaseChecker
