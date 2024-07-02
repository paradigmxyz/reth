const BaseChecker = require('../base-checker')
const { isFallbackFunction } = require('../../common/ast-types')

const ruleId = 'payable-fallback'
const meta = {
  type: 'best-practises',

  docs: {
    description: 'When fallback is not payable you will not be able to receive ethers.',
    category: 'Best Practise Rules',
    examples: {
      good: [
        {
          description: 'Fallback is payable',
          code: require('../../../test/fixtures/best-practises/fallback-payable'),
        },
      ],
      bad: [
        {
          description: 'Fallback is not payable',
          code: require('../../../test/fixtures/best-practises/fallback-not-payable'),
        },
      ],
    },
  },

  isDefault: false,
  recommended: true,
  defaultSetup: 'warn',

  schema: null,
}

class PayableFallbackChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  FunctionDefinition(node) {
    if (isFallbackFunction(node)) {
      if (node.stateMutability !== 'payable') {
        this.warn(node, 'When fallback is not payable you will not be able to receive ether')
      }
    }
  }
}

module.exports = PayableFallbackChecker
