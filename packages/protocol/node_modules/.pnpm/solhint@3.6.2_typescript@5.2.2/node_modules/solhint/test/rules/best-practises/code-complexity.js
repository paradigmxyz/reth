const linter = require('../../../lib/index')
const { funcWith, modifierWith, multiLine } = require('../../common/contract-builder')
const { assertErrorCount, assertErrorMessage, assertNoErrors } = require('../../common/asserts')

describe('Linter - code-complexity', () => {
  it('should raise error when cyclomatic complexity of a function is too high', () => {
    const report = linter.processStr(
      funcWith(require('../../fixtures/best-practises/code-complexity-high')),
      {
        rules: { 'code-complexity': 'error' },
      }
    )

    assertErrorCount(report, 1)
    assertErrorMessage(report, 'complexity')
  })

  it('should not raise error when cyclomatic complexity of a function is equal to max default allowed', () => {
    const report = linter.processStr(
      funcWith(require('../../fixtures/best-practises/code-complexity-low')),
      {
        rules: { 'code-complexity': 'error' },
      }
    )

    assertNoErrors(report)
  })

  it('should raise error when cyclomatic complexity of a modifier is too high', () => {
    const report = linter.processStr(
      modifierWith(require('../../fixtures/best-practises/code-complexity-high')),
      {
        rules: { 'code-complexity': 'error' },
      }
    )

    assertErrorCount(report, 1)
    assertErrorMessage(report, 'complexity')
  })

  const CUSTOM_CONFIG_CHECK_CODE = funcWith(
    multiLine(
      ' if (a > b) {                   ',
      '   if (b > c) {                 ',
      '     if (c > d) {               ',
      '     }                          ',
      '   }                            ',
      ' }                              ',
      'for (i = 0; i < b; i += 1) { }  ',
      'do { d++; } while (b > c);       ',
      'while (d > e) { }               ',
      'while (d > e) { }               ',
      'while (d > e) { }               ',
      'while (d > e) { }               '
    )
  )

  it('should not raise error when cyclomatic complexity is equal to max allowed', () => {
    const report = linter.processStr(CUSTOM_CONFIG_CHECK_CODE, {
      rules: { 'code-complexity': ['error', 12] },
    })

    assertNoErrors(report)
  })
})
