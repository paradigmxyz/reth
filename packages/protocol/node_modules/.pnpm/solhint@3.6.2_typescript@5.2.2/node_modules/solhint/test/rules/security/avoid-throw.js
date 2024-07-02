const assert = require('assert')
const linter = require('../../../lib/index')
const funcWith = require('../../common/contract-builder').funcWith

describe('Linter - avoid-throw', () => {
  const DEPRECATION_ERRORS = ['throw;']

  DEPRECATION_ERRORS.forEach((curText) =>
    it(`should return error that used deprecations ${curText}`, () => {
      const code = funcWith(curText)

      const report = linter.processStr(code, {
        rules: { 'avoid-throw': 'error' },
      })

      assert.equal(report.errorCount, 1)
      assert.ok(report.reports[0].message.includes('deprecate'))
    })
  )

  const ALMOST_DEPRECATION_ERRORS = ['throwing;']

  ALMOST_DEPRECATION_ERRORS.forEach((curText) =>
    it(`should not return error when doing ${curText}`, () => {
      const code = funcWith(curText)

      const report = linter.processStr(code, {
        rules: { 'avoid-throw': 'error' },
      })

      assert.equal(report.errorCount, 0)
    })
  )
})
