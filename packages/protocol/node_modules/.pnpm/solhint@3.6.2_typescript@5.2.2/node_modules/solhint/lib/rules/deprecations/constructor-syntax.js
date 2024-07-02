const BaseDeprecation = require('./base-deprecation')

const ruleId = 'constructor-syntax'
const meta = {
  type: 'best-practises',

  docs: {
    description: 'Constructors should use the new constructor keyword.',
    category: 'Best Practise Rules',
  },

  isDefault: false,
  recommended: false,
  defaultSetup: 'warn',

  schema: null,
}

class ConstructorSyntax extends BaseDeprecation {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  deprecationVersion() {
    return '0.4.22'
  }

  PragmaDirective(node) {
    super.PragmaDirective(node)
  }

  FunctionDefinition(node) {
    if (node.isConstructor) {
      if (node.name === null) {
        if (!this.active) {
          const message = 'Constructor keyword not available before 0.4.22 (' + this.version + ')'
          this.error(node, message)
        }
      } else if (this.active) {
        this.warn(node, 'Constructors should use the new constructor keyword.')
      }
    }
  }
}

module.exports = ConstructorSyntax
