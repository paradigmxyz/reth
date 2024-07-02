const assert = require('assert')
const { assertErrorMessage, assertLineNumber, assertNoErrors } = require('../../common/asserts')
const { contractWith } = require('../../common/contract-builder')
const linter = require('../../../lib/index')

describe('Linter - max-line-length', () => {
  it('should raise error when line length exceed 120', () => {
    const code = ' '.repeat(121)

    const report = linter.processStr(contractWith(code), {
      rules: { 'max-line-length': 'error' },
    })
    assert.equal(report.errorCount, 1)
    assertErrorMessage(report, 0, 'Line length must be no more than')
    assertLineNumber(report.reports[0], 6)
  })

  it('should raise error with an empty file', () => {
    const code = ' '.repeat(121)

    const report = linter.processStr(code, {
      rules: { 'max-line-length': 'error' },
    })

    assert.equal(report.errorCount, 1)
    assertErrorMessage(report, 0, 'Line length must be no more than')
  })

  it('should not raise error when line length exceed 120 and custom config provided', () => {
    const code = ' '.repeat(130)

    const report = linter.processStr(code, {
      rules: { 'max-line-length': ['error', 130] },
    })

    assertNoErrors(report)
  })

  it('should not raise error when line is exactly the max length', () => {
    const code = ' '.repeat(120)

    const report = linter.processStr(code, {
      rules: { 'max-line-length': 'error' },
    })

    assertNoErrors(report)
  })

  it('should not count newlines', () => {
    const line = ' '.repeat(120)
    const code = `${line}\n${line}\n`

    const report = linter.processStr(code, {
      rules: { 'max-line-length': 'error' },
    })

    assertNoErrors(report)
  })

  it('should not count windows newlines', () => {
    const line = ' '.repeat(120)
    const code = `${line}\n\r${line}\n\r`

    const report = linter.processStr(code, {
      rules: { 'max-line-length': 'error' },
    })

    assertNoErrors(report)
  })
})
