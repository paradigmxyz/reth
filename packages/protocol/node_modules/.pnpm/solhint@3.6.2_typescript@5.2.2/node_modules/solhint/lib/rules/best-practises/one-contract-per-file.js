const BaseChecker = require('../base-checker')
const { severityDescription } = require('../../doc/utils')

const DEFAULT_SEVERITY = 'warn'

const ruleId = 'one-contract-per-file'
const meta = {
  type: 'best-practises',

  docs: {
    description:
      'Enforces the use of ONE Contract per file see [here](https://docs.soliditylang.org/en/v0.8.21/style-guide.html#contract-and-library-names)',
    category: 'Best Practise Rules',
    options: [
      {
        description: severityDescription,
        default: DEFAULT_SEVERITY,
      },
    ],
  },

  isDefault: false,
  recommended: true,
  defaultSetup: DEFAULT_SEVERITY,

  schema: null,
}

class OneContractPerFileChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  SourceUnit(node) {
    const contractDefinitionCount = node.children.reduce((count, child) => {
      if (child.type === 'ContractDefinition') {
        return count + 1
      }
      return count
    }, 0)

    if (contractDefinitionCount > 1) {
      this.error(
        node,
        `Found more than One contract per file. ${contractDefinitionCount} contracts found!`
      )
    }
  }
}

module.exports = OneContractPerFileChecker
