const _ = require('lodash')
const BaseChecker = require('../base-checker')
const { severityDescription } = require('../../doc/utils')

const DEFAULT_SEVERITY = 'warn'
const DEFAULT_MAX_CHARACTERS_LONG = 32
const DEFAULT_OPTION = { maxLength: DEFAULT_MAX_CHARACTERS_LONG }

const ruleId = 'reason-string'
const meta = {
  type: 'best-practises',

  docs: {
    description:
      'Require or revert statement must have a reason string and check that each reason string is at most N characters long.',
    category: 'Best Practise Rules',
    options: [
      {
        description: severityDescription,
        default: DEFAULT_SEVERITY,
      },
      {
        description:
          'A JSON object with a single property "maxLength" specifying the max number of characters per reason string.',
        default: JSON.stringify(DEFAULT_OPTION),
      },
    ],
    examples: {
      good: [
        {
          description: 'Require with reason string',
          code: require('../../../test/fixtures/best-practises/require-with-reason'),
        },
      ],
      bad: [
        {
          description: 'Require without reason string',
          code: require('../../../test/fixtures/best-practises/require-without-reason'),
        },
      ],
    },
  },

  isDefault: false,
  recommended: true,
  defaultSetup: [DEFAULT_SEVERITY, DEFAULT_OPTION],

  schema: {
    type: 'object',
    properties: {
      maxLength: {
        type: 'integer',
      },
    },
  },
}

class ReasonStringChecker extends BaseChecker {
  constructor(reporter, config) {
    super(reporter, ruleId, meta)

    this.maxCharactersLong =
      (config &&
        config.getObjectPropertyNumber(ruleId, 'maxLength', DEFAULT_MAX_CHARACTERS_LONG)) ||
      DEFAULT_MAX_CHARACTERS_LONG
  }

  FunctionCall(node) {
    if (this.isReasonStringStatement(node)) {
      const functionParameters = this.getFunctionParameters(node)
      const functionName = this.getFunctionName(node)

      // Throw an error if have no message
      if (
        (functionName === 'revert' && functionParameters.length === 0) ||
        (functionName === 'require' && functionParameters.length <= 1)
      ) {
        this._errorHaveNoMessage(node, functionName)
        return
      }

      // If has reason message and is too long, throw an error
      const reason = _.last(functionParameters).value
      if (reason.length > this.maxCharactersLong) {
        const infoMessage = reason.length
          .toString()
          .concat(' counted / ')
          .concat(this.maxCharactersLong.toString())
          .concat(' allowed')
        this._errorMessageIsTooLong(node, functionName, infoMessage)
      }
    }
  }

  isReasonStringStatement(node) {
    return node.expression.name === 'revert' || node.expression.name === 'require'
  }

  getFunctionName(node) {
    return node.expression.name
  }

  getFunctionParameters(node) {
    return node.arguments
  }

  _errorHaveNoMessage(node, key) {
    this.error(node, `Provide an error message for ${key}`)
  }

  _errorMessageIsTooLong(node, key, infoMessage) {
    this.error(node, `Error message for ${key} is too long: ${infoMessage}`)
  }
}

module.exports = ReasonStringChecker
