const assert = require('assert')
const linter = require('../../../lib/index')
const contractWith = require('../../common/contract-builder').contractWith

describe('Linter - state-visibility', () => {
  it('should return required visibility error for state', () => {
    const code = contractWith('uint a;')

    const report = linter.processStr(code, {
      rules: { 'state-visibility': 'warn' },
    })

    assert.equal(report.warningCount, 1)
    assert.ok(report.reports[0].message.includes('visibility'))
  })

  const BLOCKS_WITH_DEFINITIONS = [
    contractWith('uint private a;'),
    contractWith('uint public a;'),
    contractWith('uint internal a;'),
  ]

  BLOCKS_WITH_DEFINITIONS.forEach((curData) =>
    it(`should not raise warn for blocks ${label(curData)}`, () => {
      const report = linter.processStr(curData, {
        rules: { 'state-visibility': 'warn' },
      })

      assert.equal(report.warningCount, 0)
    })
  )

  function label(data) {
    return data.split('\n')[0]
  }
})
