const _ = require('lodash')
const { assertErrorCount, assertNoErrors, assertErrorMessage } = require('../../common/asserts')
const linter = require('../../../lib/index')
const { funcWith } = require('../../common/contract-builder')

describe('Linter - function-max-lines', () => {
  it('should raise error for function with 51 lines', () => {
    const code = funcWith(emptyLines(51))

    const report = linter.processStr(code, {
      rules: { 'function-max-lines': 'error' },
    })

    assertErrorCount(report, 1)
    assertErrorMessage(report, 'no more than')
  })

  it('should not raise error for function with 50 lines', () => {
    const code = funcWith(emptyLines(50))

    const report = linter.processStr(code, {
      rules: { 'function-max-lines': 'error' },
    })

    assertNoErrors(report)
  })

  it('should not raise error for function with 99 lines with 100 allowed', () => {
    const code = funcWith(emptyLines(99))

    const report = linter.processStr(code, {
      rules: { 'function-max-lines': ['error', 100] },
    })

    assertNoErrors(report)
  })

  function repeatLines(line, count) {
    return _.times(count)
      .map(() => line)
      .join('\n')
  }

  function emptyLines(count) {
    return repeatLines('', count)
  }
})
