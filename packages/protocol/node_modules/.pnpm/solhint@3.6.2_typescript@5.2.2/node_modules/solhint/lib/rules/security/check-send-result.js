const BaseChecker = require('../base-checker')
const TreeTraversing = require('../../common/tree-traversing')

const traversing = new TreeTraversing()

const ruleId = 'check-send-result'
const meta = {
  type: 'security',

  docs: {
    description: `Check result of "send" call.`,
    category: 'Security Rules',
    examples: {
      good: [
        {
          description: 'result of "send" call checked with if statement',
          code: 'if(x.send(55)) {}',
        },
        {
          description: 'result of "send" call checked within a require',
          code: 'require(payable(walletAddress).send(moneyAmount), "Failed to send moneyAmount");',
        },
      ],
      bad: [
        {
          description: 'result of "send" call ignored',
          code: 'x.send(55);',
        },
      ],
    },
    notes: [
      {
        note: 'Rule will rise false positive on this: `bool success = walletAddress.send(amount); require(success, "Failed to send"); ` ',
      },
      { note: 'Rule will skip ERC777 "send" function to prevent false positives' },
    ],
  },

  isDefault: false,
  recommended: true,
  defaultSetup: 'warn',

  schema: null,
}

class CheckSendResultChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  MemberAccess(node) {
    this.validateSend(node)
  }

  validateSend(node) {
    if (node.memberName === 'send') {
      if (this.isNotErc777Token(node)) {
        const hasVarDeclaration = traversing.statementNotContains(node, 'VariableDeclaration')
        const hasIfStatement = traversing.statementNotContains(node, 'IfStatement')
        const hasRequire = traversing.someParent(node, this.isRequire)
        const hasAssert = traversing.someParent(node, this.isAssert)

        if (!hasIfStatement && !hasVarDeclaration && !hasRequire && !hasAssert) {
          this.error(node, 'Check result of "send" call')
        }
      }
    }
  }

  isNotErc777Token(node) {
    const isErc777 = node.parent.type === 'FunctionCall' && node.parent.arguments.length >= 3
    return !isErc777
  }

  isRequire(node) {
    return node.type === 'FunctionCall' && node.expression.name === 'require'
  }

  isAssert(node) {
    return node.type === 'FunctionCall' && node.expression.name === 'assert'
  }
}

module.exports = CheckSendResultChecker
