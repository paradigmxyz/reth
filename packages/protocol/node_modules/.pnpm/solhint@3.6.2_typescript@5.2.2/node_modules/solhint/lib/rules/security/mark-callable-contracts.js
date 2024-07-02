const TreeTraversing = require('../../common/tree-traversing')
const Reporter = require('../../reporter')

const traversing = new TreeTraversing()
const SEVERITY = Reporter.SEVERITY

const ruleId = 'mark-callable-contracts'
const meta = {
  type: 'security',

  docs: {
    description: `Explicitly mark all external contracts as trusted or untrusted.`,
    category: 'Security Rules',
    examples: {
      good: [
        {
          description: 'External contract name with "Trusted" prefix',
          code: require('../../../test/fixtures/security/external-contract-trusted'),
        },
      ],
      bad: [
        {
          description: 'External contract name without "Trusted" prefix',
          code: require('../../../test/fixtures/security/external-contract-untrusted'),
        },
      ],
    },
  },

  isDefault: false,
  recommended: false,
  defaultSetup: 'warn',
  deprecated: true,

  schema: null,
}

class MarkCallableContractsChecker {
  constructor(reporter) {
    this.reporter = reporter
    this.ruleId = ruleId
    this.meta = meta
  }

  Identifier(node) {
    const identifier = node.name
    const isFirstCharUpper = /[A-Z]/.test(identifier[0])
    const containsTrustInfo = identifier.toLowerCase().includes('trust')
    const isStatement = traversing.findParentType(node, 'ExpressionStatement')

    if (isFirstCharUpper && !containsTrustInfo && isStatement) {
      this.reporter.addMessage(
        node.loc,
        SEVERITY.WARN,
        'Explicitly mark all external contracts as trusted or untrusted',
        this.ruleId
      )
    }
  }
}

module.exports = MarkCallableContractsChecker
