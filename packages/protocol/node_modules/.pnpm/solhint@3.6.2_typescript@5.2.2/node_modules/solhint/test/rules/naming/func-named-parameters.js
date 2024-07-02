const linter = require('../../../lib/index')
const contractWith = require('../../common/contract-builder').contractWith
const {
  assertErrorCount,
  assertNoErrors,
  assertErrorMessage,
  assertWarnsCount,
} = require('../../common/asserts')
const {
  FUNCTION_CALLS_ERRORS,
  FUNCTION_CALLS_OK,
} = require('../../fixtures/naming/func-named-parameters')

const DEFAULT_MIN_UNNAMED_ARGUMENTS = 4

describe('Linter - func-named-parameters', () => {
  for (const key in FUNCTION_CALLS_ERRORS) {
    it(`should raise error for FUNCTION_CALLS_ERRORS [${key}]`, () => {
      const { code, minUnnamed } = FUNCTION_CALLS_ERRORS[key]

      const sourceCode = contractWith('function callerFunction() public { ' + code + ' }')

      const report = linter.processStr(sourceCode, {
        rules: { 'func-named-parameters': ['error', minUnnamed] },
      })

      assertErrorCount(report, 1)
      assertErrorMessage(
        report,
        `Named parameters missing. MIN unnamed argumenst is ${
          minUnnamed < DEFAULT_MIN_UNNAMED_ARGUMENTS ? DEFAULT_MIN_UNNAMED_ARGUMENTS : minUnnamed
        }`
      )
    })
  }

  for (const key in FUNCTION_CALLS_OK) {
    it(`should NOT raise error for FUNCTION_CALLS_OK [${key}]`, () => {
      const { code, minUnnamed } = FUNCTION_CALLS_OK[key]

      const sourceCode = contractWith('function callerFunction() public { ' + code + ' }')

      const report = linter.processStr(sourceCode, {
        rules: { 'func-named-parameters': ['error', minUnnamed] },
      })

      assertNoErrors(report)
    })
  }

  it('should NOT raise error when default rules are configured', () => {
    const code = contractWith(
      `function callerFunction() public { funcName(sender, amount, receiver, token1, token2); }`
    )
    const report = linter.processStr(code, {
      extends: 'solhint:default',
      rules: {},
    })

    assertNoErrors(report)
  })

  it('should NOT raise error when recommended rules are configured', () => {
    const code = contractWith(
      `function callerFunction() public { funcName(sender, amount, receiver, token1, token2); }`
    )
    const report = linter.processStr(code, {
      extends: 'solhint:recommended',
      rules: { 'compiler-version': 'off' },
    })

    assertNoErrors(report)
  })

  it('should raise warning when all rules are configured (no min value)', () => {
    const code = contractWith(
      `function callerFunction() public { funcName(sender, amount, receiver, token1, token2, token3); }`
    )

    const report = linter.processStr(code, {
      extends: 'solhint:all',
      rules: { 'compiler-version': 'off', 'comprehensive-interface': 'off' },
    })

    assertWarnsCount(report, 1)

    assertErrorMessage(
      report,
      `Named parameters missing. MIN unnamed argumenst is ${DEFAULT_MIN_UNNAMED_ARGUMENTS}`
    )
  })

  it('should raise error when rule has no minUnnamed arguments is set and default value takes over', () => {
    const code = contractWith(
      `function callerFunction() public { funcName(sender, amount, receiver, token1, token2, token3); }`
    )
    const report = linter.processStr(code, {
      rules: { 'func-named-parameters': 'error' },
    })

    assertErrorCount(report, 1)

    assertErrorMessage(
      report,
      `Named parameters missing. MIN unnamed argumenst is ${DEFAULT_MIN_UNNAMED_ARGUMENTS}`
    )
  })
})
