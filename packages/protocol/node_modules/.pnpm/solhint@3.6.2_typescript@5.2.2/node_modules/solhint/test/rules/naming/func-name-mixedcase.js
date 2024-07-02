const assert = require('assert')
const linter = require('../../../lib/index')
const contractWith = require('../../common/contract-builder').contractWith

describe('Linter - func-name-mixedcase', () => {
  it('should raise incorrect func name error', () => {
    const code = contractWith('function AFuncName () public {}')

    const report = linter.processStr(code, {
      rules: { 'func-name-mixedcase': 'error' },
    })

    assert.equal(report.errorCount, 1)
    assert.ok(report.messages[0].message.includes('mixedCase'))
  })

  it('should dot raise incorrect func name error', () => {
    const code = contractWith('function aFunc1Nam23e () public {}')

    const report = linter.processStr(code, {
      rules: { 'func-name-mixedcase': 'error' },
    })

    assert.equal(report.errorCount, 0)
  })

  describe('Function names with $ character', () => {
    const WITH_$ = {
      'starting with $': contractWith('function $aFunc1Nam23e () public {}'),
      'containing a $': contractWith('function aFunc$1Nam23e () public {}'),
      'ending with $': contractWith('function aFunc1Nam23e$ () public {}'),
      'only with $': contractWith('function $() public {}'),
    }

    for (const [key, code] of Object.entries(WITH_$)) {
      it(`should not raise func name error for Functions ${key}`, () => {
        const report = linter.processStr(code, {
          rules: { 'func-name-mixedcase': 'error' },
        })

        assert.equal(report.errorCount, 0)
      })
    }
  })
})
