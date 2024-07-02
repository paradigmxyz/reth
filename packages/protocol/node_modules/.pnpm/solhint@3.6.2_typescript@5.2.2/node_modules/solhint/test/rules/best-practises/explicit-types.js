const linter = require('../../../lib/index')
const contractWith = require('../../common/contract-builder').contractWith
const { assertErrorCount, assertNoErrors, assertErrorMessage } = require('../../common/asserts')
const VAR_DECLARATIONS = require('../../fixtures/best-practises/explicit-types')

const getZeroErrosObject = () => {
  const zeroErrorsExplicit = {}
  const zeroErrorsImplicit = {}
  for (const key in VAR_DECLARATIONS) {
    const obj = VAR_DECLARATIONS[key]
    if (obj.errorsImplicit === 0) {
      zeroErrorsImplicit[key] = obj
    }
    if (obj.errorsExplicit === 0) {
      zeroErrorsExplicit[key] = obj
    }
  }

  return { zeroErrorsImplicit, zeroErrorsExplicit }
}

describe('Linter - explicit-types rule', () => {
  it('should throw an error with a wrong configuration rule', () => {
    const code = contractWith('uint256 public constant SNAKE_CASE = 1;')

    const report = linter.processStr(code, {
      rules: { 'explicit-types': ['error', 'implicito'] },
    })

    assertErrorCount(report, 1)
    assertErrorMessage(report, `Error: Config error on [explicit-types]. See explicit-types.md.`)
  })

  it('should NOT throw error without the config array and default should take place', () => {
    const code = contractWith('uint256 public constant SNAKE_CASE = 1;')
    const report = linter.processStr(code, {
      rules: { 'explicit-types': 'error' },
    })

    assertNoErrors(report)
  })

  for (const key in VAR_DECLARATIONS) {
    it(`should raise error for ${key} on 'implicit' mode`, () => {
      const { code, errorsImplicit } = VAR_DECLARATIONS[key]
      const report = linter.processStr(contractWith(code), {
        rules: { 'explicit-types': ['error', 'implicit'] },
      })
      assertErrorCount(report, errorsImplicit)
      if (errorsImplicit > 0) assertErrorMessage(report, `Rule is set with implicit type [var/s:`)
    })
  }

  for (const key in VAR_DECLARATIONS) {
    it(`should raise error for ${key} on 'explicit' mode`, () => {
      const { code, errorsExplicit } = VAR_DECLARATIONS[key]
      const report = linter.processStr(contractWith(code), {
        rules: { 'explicit-types': ['error', 'explicit'] },
      })
      assertErrorCount(report, errorsExplicit)
      if (errorsExplicit > 0) assertErrorMessage(report, `Rule is set with explicit type [var/s:`)
    })
  }

  describe('Rule [explicit-types] - should not raise any error', () => {
    const { zeroErrorsImplicit, zeroErrorsExplicit } = getZeroErrosObject()

    for (const key in zeroErrorsExplicit) {
      it(`should NOT raise error for ${key} on 'implicit' mode`, () => {
        const { code } = zeroErrorsExplicit[key]
        const report = linter.processStr(contractWith(code), {
          rules: { 'explicit-types': ['error', 'explicit'] },
        })
        assertNoErrors(report)
      })
    }

    for (const key in zeroErrorsImplicit) {
      it(`should NOT raise error for ${key} on 'implicit' mode`, () => {
        const { code } = zeroErrorsImplicit[key]
        const report = linter.processStr(contractWith(code), {
          rules: { 'explicit-types': ['error', 'implicit'] },
        })
        assertNoErrors(report)
      })
    }
  })
})
