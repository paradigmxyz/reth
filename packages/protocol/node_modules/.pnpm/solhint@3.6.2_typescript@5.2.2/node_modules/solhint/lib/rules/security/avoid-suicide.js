const BaseChecker = require('../base-checker')

const ruleId = 'avoid-suicide'
const meta = {
  type: 'security',

  docs: {
    description: `Use "selfdestruct" instead of deprecated "suicide".`,
    category: 'Security Rules',
  },

  isDefault: false,
  recommended: true,
  defaultSetup: 'warn',

  schema: null,
}

class AvoidSuicideChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  Identifier(node) {
    if (node.name === 'suicide') {
      this.error(node, 'Use "selfdestruct" instead of deprecated "suicide"')
    }
  }
}

module.exports = AvoidSuicideChecker
