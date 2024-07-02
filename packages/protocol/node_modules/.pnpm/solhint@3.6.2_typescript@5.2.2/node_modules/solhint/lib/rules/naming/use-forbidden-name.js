const BaseChecker = require('../base-checker')

const FORBIDDEN_NAMES = ['I', 'l', 'O']

const ruleId = 'use-forbidden-name'
const meta = {
  type: 'naming',

  docs: {
    description: `Avoid to use letters 'I', 'l', 'O' as identifiers.`,
    category: 'Style Guide Rules',
  },

  isDefault: false,
  recommended: true,
  defaultSetup: 'warn',

  schema: null,
}

class UseForbiddenNameChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  VariableDeclaration(node) {
    if (FORBIDDEN_NAMES.includes(node.name)) {
      this.error(node, "Avoid to use letters 'I', 'l', 'O' as identifiers")
    }
  }
}

module.exports = UseForbiddenNameChecker
