const BaseChecker = require('../base-checker')

const ruleId = 'imports-on-top'
const meta = {
  type: 'order',

  docs: {
    description: `Import statements must be on top.`,
    category: 'Style Guide Rules',
  },

  isDefault: false,
  recommended: true,
  defaultSetup: 'warn',

  schema: null,
}

class ImportsOnTopChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  SourceUnit(node) {
    let hasContractDef = false
    for (let i = 0; node.children && i < node.children.length; i += 1) {
      const curItem = node.children[i]

      if (curItem.type === 'ContractDefinition') {
        hasContractDef = true
      }

      if (hasContractDef && curItem.type === 'ImportDirective') {
        this._error(curItem)
      }
    }
  }

  _error(node) {
    this.error(node, 'Import statements must be on top')
  }
}

module.exports = ImportsOnTopChecker
