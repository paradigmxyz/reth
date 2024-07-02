const assert = require('assert')
const linter = require('../../../lib/index')
const contractWith = require('../../common/contract-builder').contractWith

describe('immutable-vars-naming', () => {
  it('should not raise error for non immutable variables if rule is off', () => {
    const code = contractWith('uint32 private immutable D;')

    const report = linter.processStr(code, {
      rules: {
        'immutable-vars-naming': 'off',
        'var-name-mixedcase': 'error',
        'const-name-snakecase': 'error',
      },
    })

    assert.equal(report.errorCount, 0)
  })

  it('should raise error when immutablesAsConstants = false and variable is in snake case', () => {
    const code = contractWith('uint32 private immutable SNAKE_CASE;')

    const report = linter.processStr(code, {
      rules: { 'immutable-vars-naming': ['error', { immutablesAsConstants: false }] },
    })

    assert.equal(report.errorCount, 1)
    assert.ok(
      report.messages[0].message.includes('Immutable variables names are set to be in mixedCase')
    )
  })

  it('should NOT raise error when immutablesAsConstants = false and variable is in snake case', () => {
    const code = contractWith('uint32 private immutable SNAKE_CASE;')

    const report = linter.processStr(code, {
      rules: { 'immutable-vars-naming': ['error', { immutablesAsConstants: true }] },
    })

    assert.equal(report.errorCount, 0)
  })

  it('should raise error when immutablesAsConstants = true and variable is in mixedCase', () => {
    const code = contractWith('uint32 private immutable mixedCase;')

    const report = linter.processStr(code, {
      rules: { 'immutable-vars-naming': ['error', { immutablesAsConstants: true }] },
    })

    assert.equal(report.errorCount, 1)
    assert.ok(
      report.messages[0].message.includes(
        'Immutable variables name are set to be in capitalized SNAKE_CASE'
      )
    )
  })

  it('should NOT raise error when immutablesAsConstants = false and variable is in mixedCase', () => {
    const code = contractWith('uint32 private immutable mixedCase;')

    const report = linter.processStr(code, {
      rules: { 'immutable-vars-naming': ['error', { immutablesAsConstants: false }] },
    })

    assert.equal(report.errorCount, 0)
  })

  describe('Immutable variable with $ character as mixedCase', () => {
    const WITH_$ = {
      'starting with $': contractWith('uint32 immutable private $D;'),
      'containing a $': contractWith('uint32 immutable private testWith$Contained;'),
      'ending with $': contractWith('uint32 immutable private testWithEnding$;'),
      'only with $': contractWith('uint32 immutable private $;'),
    }

    for (const [key, code] of Object.entries(WITH_$)) {
      it(`should not raise immutable error for variables ${key}`, () => {
        const report = linter.processStr(code, {
          rules: { 'immutable-vars-naming': ['error', { immutablesAsConstants: false }] },
        })

        assert.equal(report.errorCount, 0)
      })
    }
  })

  describe('Immutable variable with $ character as SNAKE_CASE', () => {
    const WITH_$ = {
      'starting with $': contractWith('uint32 immutable private $_D;'),
      'starting with $D': contractWith('uint32 immutable private $D;'),
      'containing a $': contractWith('uint32 immutable private TEST_WITH_$_CONTAINED;'),
      'ending with $': contractWith('uint32 immutable private TEST_WITH_ENDING_$;'),
      'ending with D$': contractWith('uint32 immutable private TEST_WITH_ENDING_D$;'),
      'only with $': contractWith('uint32 immutable private $;'),
    }

    for (const [key, code] of Object.entries(WITH_$)) {
      it(`should not raise immutable error for variables ${key}`, () => {
        const report = linter.processStr(code, {
          rules: { 'immutable-vars-naming': ['error', { immutablesAsConstants: true }] },
        })

        assert.equal(report.errorCount, 0)
      })
    }
  })
})
