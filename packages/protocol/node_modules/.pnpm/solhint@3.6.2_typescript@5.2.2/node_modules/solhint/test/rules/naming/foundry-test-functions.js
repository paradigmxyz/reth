const assert = require('assert')
const linter = require('../../../lib/index')
const contractWith = require('../../common/contract-builder').contractWith
const { assertErrorCount, assertNoErrors, assertErrorMessage } = require('../../common/asserts')

const ALLOWED_FUNCTION_NAMES = [
  'test',
  'test_',
  'testFork_',
  'testFuzz_',
  'testFail_',
  'test_Revert_',
  'test_If_',
  'test_When_',
  'testFail_Revert_',
  'testFail_If_',
  'testFail_When_',
  'testFork_Revert_',
  'testFork_If_',
  'testFork_When_',
  'testFuzz_Revert_',
  'testFuzz_If_',
  'testFuzz_When_',
  'invariant',
  'invariant_',
  'invariantA',
  'statefulFuzz',
  'statefulFuzz_',
]

const DISALLOWED_FUNCTION_NAMES = ['Test_', 'Test', '', 'any', 'setUp', 'other', '_']

const composeFunctionName = (prefix, name, visibility) =>
  'function ' + prefix + name + ' ' + visibility + ' { testNumber = 42; }'

describe('Linter - foundry-test-functions', () => {
  for (const prefix of DISALLOWED_FUNCTION_NAMES) {
    it(`should raise error for DISALLOWED_FUNCTION_NAMES [${prefix}] when PUBLIC`, () => {
      const functionDefinition = composeFunctionName(prefix, 'FunctionName()', 'public')
      const code = contractWith(functionDefinition)

      console.log('`code` :>> ', code)
      const report = linter.processStr(code, {
        rules: { 'foundry-test-functions': ['error', ['setUp', 'finish']] },
      })

      assertErrorCount(report, 1)
      assertErrorMessage(
        report,
        `Function ${prefix + 'FunctionName()'} must match Foundry test naming convention`
      )
    })
  }

  for (const prefix of DISALLOWED_FUNCTION_NAMES) {
    it(`should NOT raise error for DISALLOWED_FUNCTION_NAMES [${prefix}] when INTERNAL`, () => {
      const functionDefinition = composeFunctionName(prefix, 'FunctionName()', 'internal')
      const code = contractWith(functionDefinition)

      const report = linter.processStr(code, {
        rules: { 'foundry-test-functions': ['error', ['setUp', 'finish']] },
      })

      assertNoErrors(report)
    })
  }

  for (const prefix of ALLOWED_FUNCTION_NAMES) {
    it(`should NOT raise error for ALLOWED_FUNCTION_NAMES [${prefix}] when PUBLIC`, () => {
      const functionDefinition = composeFunctionName(prefix, 'FunctionName()', 'public')
      const code = contractWith(functionDefinition)

      const report = linter.processStr(code, {
        rules: { 'foundry-test-functions': ['error', ['setUp', 'finish']] },
      })

      assertNoErrors(report)
    })
  }

  for (const prefix of ALLOWED_FUNCTION_NAMES) {
    it(`should NOT raise error for ALLOWED_FUNCTION_NAMES [${prefix}] when EXTERNAL`, () => {
      const functionDefinition = composeFunctionName(prefix, 'FunctionName()', 'external')
      const code = contractWith(functionDefinition)

      const report = linter.processStr(code, {
        rules: { 'foundry-test-functions': ['error', ['setUp', 'finish']] },
      })

      assertNoErrors(report)
    })
  }

  for (const prefix of ALLOWED_FUNCTION_NAMES) {
    it(`should NOT raise error for ALLOWED_FUNCTION_NAMES [${prefix}] when INTERNAL`, () => {
      const functionDefinition = composeFunctionName(prefix, 'FunctionName()', 'external')
      const code = contractWith(functionDefinition)

      const report = linter.processStr(code, {
        rules: { 'foundry-test-functions': ['error', ['setUp', 'finish']] },
      })

      assertNoErrors(report)
    })
  }

  it(`should NOT raise error for setUp function, since is confired as SKIPPED`, () => {
    const code = contractWith(
      'function setUp() public { testNumber = 42; } function finish() public { testNumber = 42; }'
    )

    const report = linter.processStr(code, {
      rules: { 'foundry-test-functions': ['error', ['setUp', 'finish']] },
    })

    assertNoErrors(report)
  })

  it(`should NOT raise error for setUp and finish functions but RAISE for the other two functions`, () => {
    const code = contractWith(`
      function setUp() public { testNumber = 42; }
      function finish() public { testNumber = 43; }
      function invalidFunction1() external { testNumber = 44; }
      function invalidFunction2() external { testNumber = 45; }`)

    const report = linter.processStr(code, {
      rules: { 'foundry-test-functions': ['error', ['setUp', 'finish']] },
    })

    assertErrorCount(report, 2)
    assert.equal(
      report.reports[0].message,
      `Function invalidFunction1() must match Foundry test naming convention`
    )
    assert.equal(
      report.reports[1].message,
      'Function invalidFunction2() must match Foundry test naming convention'
    )
  })

  it('should NOT raise error when recommended rules are configured', () => {
    const code = contractWith(`
      function setUp() public { testNumber = 42; }
      function finish() public { testNumber = 43; }
      function invalidFunction1() external { testNumber = 44; }
      function invalidFunction2() external { testNumber = 45; }`)

    const report = linter.processStr(code, {
      extends: 'solhint:recommended',
      rules: { 'compiler-version': 'off' },
    })

    assertNoErrors(report)
  })

  it('should raise 2 errors when all rules are configured and setUp is skipped', () => {
    const code = contractWith(`
      function setUp() public { testNumber = 42; }
      function invalidFunction1() external { testNumber = 44; }
      function invalidFunction2() external { testNumber = 45; }`)

    const report = linter.processStr(code, {
      extends: 'solhint:recommended',
      rules: {
        'compiler-version': 'off',
        'foundry-test-functions': ['error', ['setUp', 'finish']],
      },
    })

    assertErrorCount(report, 2)
    assert.equal(
      report.reports[0].message,
      `Function invalidFunction1() must match Foundry test naming convention`
    )
    assert.equal(
      report.reports[1].message,
      'Function invalidFunction2() must match Foundry test naming convention'
    )
  })

  it(`should NOT raise error only for setUp when rule is just on 'error' (setUp is default)`, () => {
    const code = contractWith(`
      function setUp() public { testNumber = 42; }
      function finish() public { testNumber = 43; }
      function invalidFunction1() external { testNumber = 44; }
      function invalidFunction2() external { testNumber = 45; }`)

    const report = linter.processStr(code, {
      rules: { 'foundry-test-functions': 'error' },
    })

    assertErrorCount(report, 3)
    assert.equal(
      report.reports[0].message,
      'Function finish() must match Foundry test naming convention'
    )
    assert.equal(
      report.reports[1].message,
      `Function invalidFunction1() must match Foundry test naming convention`
    )
    assert.equal(
      report.reports[2].message,
      'Function invalidFunction2() must match Foundry test naming convention'
    )
  })

  it(`should raise error for all functions when rule SKIP array is empty`, () => {
    const code = contractWith(`
      function setUp() public { testNumber = 42; }
      function finish() public { testNumber = 43; }
      function invalidFunction() external { testNumber = 44; }`)

    const report = linter.processStr(code, {
      rules: { 'foundry-test-functions': ['error', []] },
    })

    assertErrorCount(report, 3)
    assert.equal(
      report.reports[0].message,
      'Function setUp() must match Foundry test naming convention'
    )
    assert.equal(
      report.reports[1].message,
      `Function finish() must match Foundry test naming convention`
    )
    assert.equal(
      report.reports[2].message,
      'Function invalidFunction() must match Foundry test naming convention'
    )
  })
})
