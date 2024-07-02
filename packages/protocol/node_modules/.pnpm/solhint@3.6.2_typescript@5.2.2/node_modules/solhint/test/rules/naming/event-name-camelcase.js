const assert = require('assert')
const linter = require('../../../lib/index')
const contractWith = require('../../common/contract-builder').contractWith

describe('Linter - event-name-camelcase', () => {
  it('should raise event name error for event in mixedCase', () => {
    const code = contractWith('event event1(uint a);')

    const report = linter.processStr(code, {
      rules: { 'event-name-camelcase': 'error' },
    })

    assert.equal(report.errorCount, 1)
    assert.ok(report.messages[0].message.includes('CamelCase'))
  })

  describe('Event name with $ character', () => {
    const WITH_$ = {
      'starting with $': contractWith('event $Event1(uint a);'),
      'containing a $': contractWith('event Eve$nt1(uint a);'),
      'ending with $': contractWith('event Event1$(uint a);'),
      'only with $': contractWith('event $(uint a);'),
    }

    for (const [key, code] of Object.entries(WITH_$)) {
      it(`should not raise event name error for Events ${key}`, () => {
        const report = linter.processStr(code, {
          rules: { 'event-name-camelcase': 'error' },
        })

        assert.equal(report.errorCount, 0)
      })
    }
  })
})
