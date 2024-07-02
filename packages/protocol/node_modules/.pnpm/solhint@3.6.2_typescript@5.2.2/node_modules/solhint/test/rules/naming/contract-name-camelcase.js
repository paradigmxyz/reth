const assert = require('assert')
const linter = require('../../../lib/index')
const contractWith = require('../../common/contract-builder').contractWith

describe('Linter - contract-name-camelcase', () => {
  it('should raise struct name error', () => {
    const code = contractWith('struct a {}')

    const report = linter.processStr(code, {
      rules: { 'contract-name-camelcase': 'error' },
    })

    assert.equal(report.errorCount, 1)
    assert.ok(report.messages[0].message.includes('CamelCase'))
  })

  it('should raise contract name error', () => {
    const code = 'contract a {}'

    const report = linter.processStr(code, {
      rules: { 'contract-name-camelcase': 'error' },
    })

    assert.equal(report.errorCount, 1)
    assert.ok(report.messages[0].message.includes('CamelCase'))
  })

  it('should raise enum name error', () => {
    const code = contractWith('enum abc {}')

    const report = linter.processStr(code, {
      rules: { 'contract-name-camelcase': 'error' },
    })

    assert.equal(report.errorCount, 1)
    assert.ok(report.messages[0].message.includes('CamelCase'))
  })

  describe('Struct name with $ character', () => {
    const WITH_$ = {
      'starting with $': contractWith('struct $MyStruct {}'),
      'containing a $': contractWith('struct My$Struct {}'),
      'ending with $': contractWith('struct MyStruct$ {}'),
      'only with $': contractWith('struct $ {}'),
    }

    for (const [key, code] of Object.entries(WITH_$)) {
      it(`should not raise contract name error for Structs ${key}`, () => {
        const report = linter.processStr(code, {
          rules: { 'contract-name-camelcase': 'error' },
        })

        assert.equal(report.errorCount, 0)
      })
    }
  })

  describe('Enums name with $ character', () => {
    const WITH_$ = {
      'starting with $': contractWith('enum $MyEnum {}'),
      'containing a $': contractWith('enum My$Enum {}'),
      'ending with $': contractWith('enum MyEnum$ {}'),
      'only with $': contractWith('enum $ {}'),
    }

    for (const [key, code] of Object.entries(WITH_$)) {
      it(`should not raise contract name error for Enums ${key}`, () => {
        const report = linter.processStr(code, {
          rules: { 'contract-name-camelcase': 'error' },
        })

        assert.equal(report.errorCount, 0)
      })
    }
  })

  describe('Contract name with $ character', () => {
    const WITH_$ = {
      'starting with $': 'contract $MyContract {}',
      'containing a $': 'contract My$Contract {}',
      'ending with $': 'contract MyContract$ {}',
      'only with $': 'contract $ {}',
    }

    for (const [key, code] of Object.entries(WITH_$)) {
      it(`should not raise contract name error for Contracts ${key}`, () => {
        const report = linter.processStr(code, {
          rules: { 'contract-name-camelcase': 'error' },
        })

        assert.equal(report.errorCount, 0)
      })
    }
  })
})
