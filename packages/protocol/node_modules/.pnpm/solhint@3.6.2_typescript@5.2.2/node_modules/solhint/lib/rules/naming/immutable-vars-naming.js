const BaseChecker = require('../base-checker')
const naming = require('../../common/identifier-naming')
const { severityDescription } = require('../../doc/utils')

const DEFAULT_INMUTABLE_AS_CONSTANTS = true
const DEFAULT_SEVERITY = 'warn'
const DEFAULT_OPTION = { immutablesAsConstants: DEFAULT_INMUTABLE_AS_CONSTANTS }

const ruleId = 'immutable-vars-naming'
const meta = {
  type: 'naming',

  docs: {
    description:
      'Check Immutable variables. Capitalized SNAKE_CASE or mixedCase depending on configuration.',
    category: 'Style Guide Rules',
    options: [
      {
        description: severityDescription,
        default: DEFAULT_SEVERITY,
      },
      {
        description:
          'A JSON object with a single property "immutablesAsConstants" as boolean specifying if immutable variables should be treated as constants',
        default: JSON.stringify(DEFAULT_OPTION),
      },
    ],
  },

  isDefault: false,
  recommended: true,
  defaultSetup: [DEFAULT_SEVERITY, DEFAULT_OPTION],

  schema: {
    type: 'object',
    properties: {
      immutablesAsConstants: {
        type: 'boolean',
      },
    },
  },
}

class ImmutableVarsNamingChecker extends BaseChecker {
  constructor(reporter, config) {
    super(reporter, ruleId, meta)

    this.treatImmutablesAsConstants =
      config &&
      config.getObjectPropertyBoolean(
        ruleId,
        'immutablesAsConstants',
        DEFAULT_INMUTABLE_AS_CONSTANTS
      )
  }

  VariableDeclaration(node) {
    if (node.isImmutable) {
      if (this.treatImmutablesAsConstants) {
        this.validateImmutableAsConstantName(node)
      } else {
        this.validateImmutableAsRegularVariables(node)
      }
    }
  }

  validateImmutableAsConstantName(node) {
    if (naming.isNotUpperSnakeCase(node.name)) {
      this.error(node, 'Immutable variables name are set to be in capitalized SNAKE_CASE')
    }
  }

  validateImmutableAsRegularVariables(node) {
    if (naming.isNotMixedCase(node.name)) {
      this.error(node, 'Immutable variables names are set to be in mixedCase')
    }
  }
}

module.exports = ImmutableVarsNamingChecker
