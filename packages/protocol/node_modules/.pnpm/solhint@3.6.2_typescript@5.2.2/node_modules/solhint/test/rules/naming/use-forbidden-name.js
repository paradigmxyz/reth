const assert = require('assert')
const linter = require('../../../lib/index')
const funcWith = require('../../common/contract-builder').funcWith

describe('Linter - use-forbidden-name', () => {
  it('should raise forbidden name error', () => {
    const code = funcWith('uint l = 0;')

    const report = linter.processStr(code, {
      rules: { 'no-unused-vars': 'error', 'use-forbidden-name': 'error' },
    })

    assert.equal(report.errorCount, 2)
    assert.ok(report.messages[0].message.includes('Avoid to use'))
  })
})
