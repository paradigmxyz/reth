const BaseChecker = require('../base-checker')

const ruleId = 'no-global-import'
const meta = {
  type: 'best-practises',

  docs: {
    description: 'Import statement includes an entire file instead of selected symbols.',
    category: 'Best Practise Rules',
    examples: {
      bad: [
        { description: 'import all members from a file', code: 'import * from "foo.sol"' },
        { description: 'import an entire file', code: 'import "foo.sol"' },
      ],
      good: [
        { description: 'import names explicitly', code: 'import {A} from "./A.sol"' },
        { description: 'import entire file into a name', code: 'import "./A.sol" as A' },
        { description: 'import entire file into a name', code: 'import * as A from "./A.sol"' },
      ],
    },
  },

  isDefault: false,
  recommended: true,
  defaultSetup: 'warn',

  schema: null,
}

class NoGlobalImportsChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  ImportDirective(node) {
    if (!(node.symbolAliases || node.unitAlias)) {
      this.error(
        node,
        `global import of path ${node.path} is not allowed. Specify names to import individually or bind all exports of the module into a name (import "path" as Name)`
      )
    }
  }
}

module.exports = NoGlobalImportsChecker
