const BaseChecker = require('../base-checker')

const ruleId = 'comprehensive-interface'
const meta = {
  type: 'miscellaneous',

  docs: {
    description:
      'Check that all public or external functions are override. This is iseful to make sure that the whole API is extracted in an interface.',
    category: 'Miscellaneous',
    examples: {
      good: [
        {
          description: 'All public functions are overrides',
          code: require('../../../test/fixtures/miscellaneous/public-function-with-override'),
        },
      ],
      bad: [
        {
          description: 'A public function is not an override',
          code: require('../../../test/fixtures/miscellaneous/public-function-no-override'),
        },
      ],
    },
  },

  isDefault: false,
  recommended: false,
  defaultSetup: 'warn',

  schema: null,
}

class ComprehensiveInterface extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  ContractDefinition(node) {
    this.isContract = node.kind === 'contract'
  }

  'ContractDefinition:exit'() {
    this.isContract = false
  }

  FunctionDefinition(node) {
    if (!this.isContract) {
      return
    }

    const isPublic = node.visibility === 'public' || node.visibility === 'external'
    const isOverride = node.override !== null

    if (isPublic && !isOverride) {
      this.error(
        node,
        'All public or external methods in a contract must override a definition from an interface'
      )
    }
  }
}

module.exports = ComprehensiveInterface
