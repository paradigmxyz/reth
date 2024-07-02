const BaseChecker = require('../base-checker')
const naming = require('../../common/identifier-naming')

const ruleId = 'contract-name-camelcase'
const meta = {
  type: 'naming',

  docs: {
    description: 'Contract name must be in CamelCase.',
    category: 'Style Guide Rules',
  },

  isDefault: false,
  recommended: true,
  defaultSetup: 'warn',

  schema: null,
}

class ContractNameCamelcaseChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  ContractDefinition(node) {
    this.validateName(node)
  }

  EnumDefinition(node) {
    this.validateName(node)
  }

  StructDefinition(node) {
    this.validateName(node)
  }

  validateName(node) {
    if (naming.isNotCamelCase(node.name)) {
      this._error(node)
    }
  }

  _error(node) {
    this.error(node, 'Contract name must be in CamelCase')
  }
}

module.exports = ContractNameCamelcaseChecker
