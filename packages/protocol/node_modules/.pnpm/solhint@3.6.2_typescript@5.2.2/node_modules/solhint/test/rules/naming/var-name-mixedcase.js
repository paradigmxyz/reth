const assert = require('assert')
const linter = require('../../../lib/index')
const contractWith = require('../../common/contract-builder').contractWith
const funcWith = require('../../common/contract-builder').funcWith

describe('Linter - var-name-mixedcase', () => {
  it('should raise incorrect var name error', () => {
    const code = funcWith('var (a, B);')

    const report = linter.processStr(code, {
      rules: { 'no-unused-vars': 'error', 'var-name-mixedcase': 'error' },
    })

    assert.ok(report.errorCount > 0)
    assert.ok(report.messages.map((i) => i.message).some((i) => i.includes('name')))
  })

  it('should raise incorrect var name error for typed declaration', () => {
    const code = funcWith('uint B = 1;')

    const report = linter.processStr(code, {
      rules: { 'no-unused-vars': 'error', 'var-name-mixedcase': 'error' },
    })

    assert.ok(report.errorCount > 0)
    assert.ok(report.messages.map((i) => i.message).some((i) => i.includes('name')))
  })

  it('should raise incorrect var name error for state declaration', () => {
    const code = contractWith('uint32 private D = 10;')

    const report = linter.processStr(code, {
      rules: { 'no-unused-vars': 'error', 'var-name-mixedcase': 'error' },
    })

    assert.equal(report.errorCount, 1)
    assert.ok(report.messages[0].message.includes('Variable name'))
  })

  it('should not raise var name error for constants', () => {
    const code = contractWith('uint32 private constant D = 10;')

    const report = linter.processStr(code, {
      rules: { 'no-unused-vars': 'error', 'var-name-mixedcase': 'error' },
    })

    assert.equal(report.errorCount, 0)
  })

  it('should not raise const name error for immutable variables in SNAKE_CASE', () => {
    const code = contractWith('uint32 private immutable SNAKE_CASE;')

    const report = linter.processStr(code, {
      rules: { 'no-unused-vars': 'error', 'var-name-mixedcase': 'error' },
    })

    assert.equal(report.errorCount, 0)
  })

  it('should not raise const name error for immutable variables in mixedCase', () => {
    const code = contractWith('uint32 private immutable SNAKE_CASE;')

    const report = linter.processStr(code, {
      rules: { 'no-unused-vars': 'error', 'var-name-mixedcase': 'error' },
    })

    assert.equal(report.errorCount, 0)
  })

  describe('with $ character', () => {
    const WITH_$ = {
      'starting with $': contractWith('uint32 private $D = 10;'),
      'containing a $': contractWith('uint32 private testWith$Contained = 10;'),
      'ending with $': contractWith('uint32 private testWithEnding$ = 10;'),
      'only with $': contractWith('uint32 private $;'),
    }

    for (const [key, code] of Object.entries(WITH_$)) {
      it(`should not raise var name error for variables ${key}`, () => {
        const report = linter.processStr(code, {
          rules: { 'var-name-mixedcase': 'error' },
        })

        assert.equal(report.errorCount, 0)
      })
    }
  })
})
