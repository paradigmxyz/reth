const BaseChecker = require('../base-checker')
const { severityDescription } = require('../../doc/utils')

const DEFAULT_SEVERITY = 'warn'
const DEFAULT_IGNORE_CONSTRUCTORS = false
const DEFAULT_OPTION = { ignoreConstructors: DEFAULT_IGNORE_CONSTRUCTORS }

const ruleId = 'func-visibility'
const meta = {
  type: 'security',

  docs: {
    description: `Explicitly mark visibility in function.`,
    category: 'Security Rules',
    options: [
      {
        description: severityDescription,
        default: DEFAULT_SEVERITY,
      },
      {
        description:
          'A JSON object with a single property "ignoreConstructors" specifying if the rule should ignore constructors. (**Note: This is required to be true for Solidity >=0.7.0 and false for <0.7.0**)',
        default: JSON.stringify(DEFAULT_OPTION),
      },
    ],
    examples: {
      good: [
        {
          description: 'Functions explicitly marked with visibility',
          code: require('../../../test/fixtures/security/functions-with-visibility').join('\n'),
        },
      ],
      bad: [
        {
          description: 'Functions without explicitly marked visibility',
          code: require('../../../test/fixtures/security/functions-without-visibility').join('\n'),
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
      ignoreConstructors: {
        type: 'boolean',
      },
    },
  },
}

class FuncVisibilityChecker extends BaseChecker {
  constructor(reporter, config) {
    super(reporter, ruleId, meta)

    this.ignoreConstructors =
      config && config.getObjectPropertyBoolean(ruleId, 'ignoreConstructors', false)
  }

  ContractDefinition() {
    this.isContract = true
  }

  'ContractDefinition:exit'() {
    this.isContract = false
  }

  FunctionDefinition(node) {
    if (node.isConstructor && this.ignoreConstructors) {
      return
    }

    if (this.isContract) {
      if (node.visibility === 'default') {
        this.warn(
          node,
          node.isConstructor
            ? 'Explicitly mark visibility in function (Set ignoreConstructors to true if using solidity >=0.7.0)'
            : 'Explicitly mark visibility in function'
        )
      }
    }
  }
}

module.exports = FuncVisibilityChecker
