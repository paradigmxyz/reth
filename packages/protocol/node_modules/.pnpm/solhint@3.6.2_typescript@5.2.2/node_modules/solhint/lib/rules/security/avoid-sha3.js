const BaseChecker = require('../base-checker')

const ruleId = 'avoid-sha3'
const meta = {
  type: 'security',

  docs: {
    description: `Use "keccak256" instead of deprecated "sha3".`,
    category: 'Security Rules',
  },

  isDefault: false,
  recommended: true,
  defaultSetup: 'warn',
  fixable: true,

  schema: null,
}

class AvoidSha3Checker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  Identifier(node) {
    if (node.name === 'sha3') {
      this.error(node, 'Use "keccak256" instead of deprecated "sha3"', (fixer) =>
        fixer.replaceTextRange(node.range, 'keccak256')
      )
    }
  }
}

module.exports = AvoidSha3Checker
