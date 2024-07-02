const BaseChecker = require('../base-checker')
const naming = require('../../common/identifier-naming')
const { severityDescription } = require('../../doc/utils')

const DEFAULT_SEVERITY = 'off'
const DEFAULT_SKIP_FUNCTIONS = ['setUp']

const ruleId = 'foundry-test-functions'
const meta = {
  type: 'naming',

  docs: {
    description: `Enforce naming convention on functions for Foundry test cases`,
    category: 'Style Guide Rules',
    options: [
      {
        description: severityDescription,
        default: DEFAULT_SEVERITY,
      },
    ],
    examples: {
      good: [
        {
          description: 'Foundry test case with correct Function declaration',
          code: 'function test_NumberIs42() public {}',
        },
        {
          description: 'Foundry test case with correct Function declaration',
          code: 'function testFail_Subtract43() public {}',
        },
        {
          description: 'Foundry test case with correct Function declaration',
          code: 'function testFuzz_FuzzyTest() public {}',
        },
      ],
      bad: [
        {
          description: 'Foundry test case with incorrect Function declaration',
          code: 'function numberIs42() public {}',
        },
      ],
    },
    notes: [
      {
        note: 'This rule can be configured to skip certain function names in the SKIP array. In Example Config. ```setUp``` function will be skipped',
      },
      { note: 'Supported Regex: ```test(Fork)?(Fuzz)?(Fail)?_(Revert(If_|When_){1})?\\w{1,}```' },
      {
        note: 'This rule should be executed in a separate folder with a separate .solhint.json => ```solhint --config .solhint.json testFolder/**/*.sol```',
      },
      {
        note: 'This rule applies only to `external` and `public` functions',
      },
      {
        note: 'This rule skips the `setUp()` function by default',
      },
    ],
  },

  isDefault: false,
  recommended: false,
  defaultSetup: [DEFAULT_SEVERITY, DEFAULT_SKIP_FUNCTIONS],

  schema: { type: 'array' },
}

class FoundryTestFunctionsChecker extends BaseChecker {
  constructor(reporter, config) {
    super(reporter, ruleId, meta)
    this.skippedFunctions = config
      ? config.getArray(ruleId, DEFAULT_SKIP_FUNCTIONS)
      : DEFAULT_SKIP_FUNCTIONS
  }

  FunctionDefinition(node) {
    // function name should not be in skipped functions array
    // should be external or public
    if (
      !this.searchInArray(this.skippedFunctions, node.name) &&
      (node.visibility === 'public' || node.visibility === 'external')
    ) {
      if (!naming.isFoundryTestCase(node.name)) {
        this.error(node, `Function ${node.name}() must match Foundry test naming convention`)
      }
    }
  }

  searchInArray(array, searchString) {
    return array.indexOf(searchString) !== -1
  }
}

module.exports = FoundryTestFunctionsChecker
