const BaseChecker = require('../base-checker')
const naming = require('../../common/identifier-naming')

const ruleId = 'const-name-snakecase'
const meta = {
  type: 'naming',

  docs: {
    description:
      'Constant name must be in capitalized SNAKE_CASE. (Does not check IMMUTABLES, use immutable-vars-naming)',
    category: 'Style Guide Rules',
  },

  isDefault: false,
  recommended: true,
  defaultSetup: 'warn',

  schema: null,
}

class ConstNameSnakecaseChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  VariableDeclaration(node) {
    if (node.isDeclaredConst) {
      this.validateConstantName(node)
    }
  }

  validateConstantName(variable) {
    if (naming.isNotUpperSnakeCase(variable.name)) {
      this.error(variable, 'Constant name must be in capitalized SNAKE_CASE')
    }
  }
}

module.exports = ConstNameSnakecaseChecker
