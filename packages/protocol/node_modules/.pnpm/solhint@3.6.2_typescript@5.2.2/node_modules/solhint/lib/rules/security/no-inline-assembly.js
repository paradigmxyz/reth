const BaseChecker = require('../base-checker')

const ruleId = 'no-inline-assembly'
const meta = {
  type: 'security',

  docs: {
    description: `Avoid to use inline assembly. It is acceptable only in rare cases.`,
    category: 'Security Rules',
  },

  isDefault: false,
  recommended: true,
  defaultSetup: 'warn',

  schema: null,
}

class NoInlineAssemblyChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  InlineAssemblyStatement(node) {
    this.error(node)
  }

  error(node) {
    this.warn(node, 'Avoid to use inline assembly. It is acceptable only in rare cases')
  }
}

module.exports = NoInlineAssemblyChecker
