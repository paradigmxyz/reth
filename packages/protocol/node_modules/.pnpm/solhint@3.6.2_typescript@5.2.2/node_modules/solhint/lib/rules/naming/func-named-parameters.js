const BaseChecker = require('../base-checker')
const { severityDescription } = require('../../doc/utils')

const DEFAULT_SEVERITY = 'warn'
const DEFAULT_MIN_UNNAMED_ARGUMENTS = 4

const ruleId = 'func-named-parameters'
const meta = {
  type: 'naming',

  docs: {
    description: `Enforce named parameters for function calls with ${DEFAULT_MIN_UNNAMED_ARGUMENTS} or more arguments. This rule may have some false positives`,
    category: 'Style Guide Rules',
    options: [
      {
        description: severityDescription,
        default: DEFAULT_SEVERITY,
      },
      {
        description:
          'Minimum qty of unnamed parameters for a function call (to prevent false positives on builtin functions).',
        default: JSON.stringify(DEFAULT_MIN_UNNAMED_ARGUMENTS),
      },
    ],
    examples: {
      good: [
        {
          description: 'Function call with two UNNAMED parameters (default is 4)',
          code: "functionName('0xA81705c8C247C413a19A244938ae7f4A0393944e', 1e18)",
        },
        {
          description: 'Function call with two NAMED parameters',
          code: "functionName({ sender: '0xA81705c8C247C413a19A244938ae7f4A0393944e', amount: 1e18})",
        },
        {
          description: 'Function call with four NAMED parameters',
          code: 'functionName({ sender: _senderAddress, amount: 1e18, token: _tokenAddress, receiver: _receiverAddress })',
        },
      ],
      bad: [
        {
          description: 'Function call with four UNNAMED parameters (default 4)',
          code: 'functionName(_senderAddress, 1e18, _tokenAddress, _receiverAddress )',
        },
      ],
    },
  },

  isDefault: false,
  recommended: false,
  defaultSetup: [DEFAULT_SEVERITY, DEFAULT_MIN_UNNAMED_ARGUMENTS],

  schema: { type: 'integer' },
}

class FunctionNamedParametersChecker extends BaseChecker {
  constructor(reporter, config) {
    super(reporter, ruleId, meta)
    this.maxUnnamedArguments =
      (config && config.getNumber(ruleId, DEFAULT_MIN_UNNAMED_ARGUMENTS)) ||
      DEFAULT_MIN_UNNAMED_ARGUMENTS
    if (this.maxUnnamedArguments < DEFAULT_MIN_UNNAMED_ARGUMENTS)
      this.maxUnnamedArguments = DEFAULT_MIN_UNNAMED_ARGUMENTS
  }

  FunctionCall(node) {
    const qtyNamed = node.names.length
    const qtyArgs = node.arguments.length

    if (qtyArgs !== 0) {
      if (qtyNamed === 0 && qtyArgs > this.maxUnnamedArguments) {
        this.error(
          node,
          `Named parameters missing. MIN unnamed argumenst is ${this.maxUnnamedArguments}`
        )
      }
    }
  }
}

module.exports = FunctionNamedParametersChecker
