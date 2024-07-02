const BaseChecker = require('../base-checker')
const { isFallbackFunction } = require('../../common/ast-types')

const ruleId = 'no-complex-fallback'
const meta = {
  type: 'security',

  docs: {
    description: `Fallback function must be simple.`,
    category: 'Security Rules',
  },

  isDefault: false,
  recommended: true,
  defaultSetup: 'warn',

  schema: null,
}

class NoComplexFallbackChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  FunctionDefinition(node) {
    if (isFallbackFunction(node)) {
      if (node.body.statements.length >= 2) {
        this.warn(node, 'Fallback function must be simple')
      }
    }
  }
}

module.exports = NoComplexFallbackChecker
