const BaseChecker = require('../base-checker')

const ruleId = 'no-console'
const meta = {
  type: 'best-practises',
  docs: {
    description:
      'No console.log/logInt/logBytesX/logString/etc & No hardhat and forge-std console.sol import statements.',
    category: 'Best Practise Rules',
    examples: {
      bad: [
        {
          description: 'No console.logX statements',
          code: "console.log('test')",
        },
        {
          description: 'No hardhat/console.sol import statements',
          code: 'import "hardhat/console.sol"',
        },
        {
          description: 'No forge-std console.sol & console2.sol import statements',
          code: 'import "forge-std/consoleN.sol"',
        },
      ],
    },
  },

  isDefault: true,
  recommended: true,
  defaultSetup: 'error',
  schema: null,
}

class NoConsoleLogChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  ImportDirective(node) {
    if (this.isConsoleImport(node)) {
      this.error(node, 'Unexpected import of console file')
    }
  }

  FunctionCall(node) {
    if (this.isConsoleLog(node)) {
      this.error(node, 'Unexpected console statement')
    }
  }

  isConsoleLog(node) {
    return (
      node.expression.expression.type === 'Identifier' &&
      (node.expression.expression.name === 'console' ||
        node.expression.expression.name === 'console2') &&
      node.expression.memberName.startsWith('log')
    )
  }

  isConsoleImport(node) {
    return (
      node.path === 'hardhat/console.sol' ||
      node.path === 'forge-std/console.sol' ||
      node.path === 'forge-std/console2.sol'
    )
  }
}

module.exports = NoConsoleLogChecker
