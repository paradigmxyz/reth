const BaseChecker = require('../base-checker')

const ruleId = 'state-visibility'
const meta = {
  type: 'security',

  docs: {
    description: `Explicitly mark visibility of state.`,
    category: 'Security Rules',
  },

  isDefault: false,
  recommended: true,
  defaultSetup: 'warn',

  schema: null,
}

class StateVisibilityChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  StateVariableDeclaration(node) {
    if (node.variables.some(({ visibility }) => visibility === 'default')) {
      this.warn(node, 'Explicitly mark visibility of state')
    }
  }
}

module.exports = StateVisibilityChecker
