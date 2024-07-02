const BaseChecker = require('../base-checker')

const ruleId = 'avoid-throw'
const meta = {
  type: 'security',

  docs: {
    description: `"throw" is deprecated, avoid to use it.`,
    category: 'Security Rules',
  },

  isDefault: false,
  recommended: true,
  defaultSetup: 'warn',
  fixable: true,

  schema: null,
}

class AvoidThrowChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  ThrowStatement(node) {
    this.error(node, '"throw" is deprecated, avoid to use it', (fixer) =>
      // we don't use just `node.range` because ThrowStatement includes the semicolon and the spaces before it
      // we know that node.range[0] is the 't' of throw
      // we're also pretty sure that 'throw' has 5 letters
      fixer.replaceTextRange([node.range[0], node.range[0] + 5], 'revert();')
    )
  }
}

module.exports = AvoidThrowChecker
