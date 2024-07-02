const _ = require('lodash')
const BaseChecker = require('../base-checker')
const { severityDescription } = require('../../doc/utils')

const ruleId = 'max-states-count'
const DEFAULT_SEVERITY = 'warn'
const DEFAULT_MAX_STATES_COUNT = 15
const meta = {
  type: 'best-practises',

  docs: {
    description:
      'Contract has "some count" states declarations but allowed no more than maxstates.',
    category: 'Best Practise Rules',
    options: [
      {
        description: severityDescription,
        default: DEFAULT_SEVERITY,
      },
      {
        description: 'Maximum allowed number of states declarations',
        default: DEFAULT_MAX_STATES_COUNT,
      },
    ],
    examples: {
      good: [
        {
          description: 'Low number of states',
          code: require('../../../test/fixtures/best-practises/number-of-states-low'),
        },
      ],
      bad: [
        {
          description: 'High number of states',
          code: require('../../../test/fixtures/best-practises/number-of-states-high'),
        },
      ],
    },
  },

  isDefault: false,
  recommended: true,
  defaultSetup: [DEFAULT_SEVERITY, DEFAULT_MAX_STATES_COUNT],

  schema: { type: 'integer' },
}

class MaxStatesCountChecker extends BaseChecker {
  constructor(reporter, config) {
    super(reporter, ruleId, meta)

    this.maxStatesCount =
      (config && config.getNumber(ruleId, DEFAULT_MAX_STATES_COUNT)) || DEFAULT_MAX_STATES_COUNT
  }

  ContractDefinition(node) {
    const countOfVars = _(node.subNodes)
      .filter(({ type }) => type === 'StateVariableDeclaration')
      .flatMap(({ variables }) =>
        variables.filter(({ isDeclaredConst, isImmutable }) => !isDeclaredConst && !isImmutable)
      )
      .value().length

    if (countOfVars > this.maxStatesCount) {
      this._error(node, countOfVars)
    }
  }

  _error(node, countOfVars) {
    const curStatesCount = countOfVars
    const maxStatesCount = this.maxStatesCount

    const message = `Contract has ${curStatesCount} states declarations but allowed no more than ${maxStatesCount}`
    this.error(node, message)
  }
}

module.exports = MaxStatesCountChecker
