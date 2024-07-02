const BaseChecker = require('../base-checker')
const { severityDescription } = require('../../doc/utils')

const DEFAULT_SEVERITY = 'warn'

const ruleId = 'custom-errors'
const meta = {
  type: 'best-practises',

  docs: {
    description: 'Enforces the use of Custom Errors over Require and Revert statements',
    category: 'Best Practise Rules',
    options: [
      {
        description: severityDescription,
        default: DEFAULT_SEVERITY,
      },
    ],
    examples: {
      good: [
        {
          description: 'Use of Custom Errors',
          code: 'revert CustomErrorFunction();',
        },
        {
          description: 'Use of Custom Errors with arguments',
          code: 'revert CustomErrorFunction({ msg: "Insufficient Balance" });',
        },
      ],
      bad: [
        {
          description: 'Use of require statement',
          code: 'require(userBalance >= availableAmount, "Insufficient Balance");',
        },
        {
          description: 'Use of plain revert statement',
          code: 'revert();',
        },
        {
          description: 'Use of revert statement with message',
          code: 'revert("Insufficient Balance");',
        },
      ],
    },
  },

  isDefault: false,
  recommended: true,
  defaultSetup: DEFAULT_SEVERITY,

  schema: null,
}

class CustomErrorsChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  FunctionCall(node) {
    let errorStr = ''
    if (node.expression.name === 'require') {
      errorStr = 'require'
    } else if (
      node.expression.name === 'revert' &&
      (node.arguments.length === 0 || node.arguments[0].type === 'StringLiteral')
    ) {
      errorStr = 'revert'
    }

    if (errorStr !== '') {
      this.error(node, `Use Custom Errors instead of ${errorStr} statements`)
    }
  }
}

module.exports = CustomErrorsChecker
