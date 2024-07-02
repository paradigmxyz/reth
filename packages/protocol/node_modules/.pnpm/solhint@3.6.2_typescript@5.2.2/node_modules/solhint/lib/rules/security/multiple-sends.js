const BaseChecker = require('../base-checker')
const TreeTraversing = require('../../common/tree-traversing')

const traversing = new TreeTraversing()

const ruleId = 'multiple-sends'
const meta = {
  type: 'security',

  docs: {
    description: `Avoid multiple calls of "send" method in single transaction.`,
    category: 'Security Rules',
  },

  isDefault: false,
  recommended: true,
  defaultSetup: 'warn',

  schema: null,
}

class MultipleSendsChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
    this.funcDefSet = new Set()
  }

  MemberAccess(node) {
    const isOk = this.validateMultipleSendInFunc(node)

    if (isOk) {
      this.validateSendCallInLoop(node)
    }
  }

  validateMultipleSendInFunc(node) {
    if (node.memberName === 'send') {
      const curFuncDef = traversing.findParentType(node, 'FunctionDefinition')

      if (curFuncDef) {
        if (this.funcDefSet.has(curFuncDef.name)) {
          this._error(node)
          return false
        } else {
          this.funcDefSet.add(curFuncDef.name)
        }
      }
    }

    return true
  }

  validateSendCallInLoop(node) {
    if (node.memberName === 'send') {
      const hasForLoop = traversing.findParentType(node, 'ForStatement') !== null
      const hasWhileLoop = traversing.findParentType(node, 'WhileStatement') !== null
      const hasDoWhileLoop = traversing.findParentType(node, 'DoWhileStatement') !== null

      if (hasForLoop || hasWhileLoop || hasDoWhileLoop) {
        this._error(node)
      }
    }
  }

  _error(node) {
    this.error(node, 'Avoid multiple calls of "send" method in single transaction')
  }
}

module.exports = MultipleSendsChecker
