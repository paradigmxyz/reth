const assert = require('assert')
const _ = require('lodash')
const {
  validate,
  validSeverityMap,
  defaultSchemaValueForRules,
} = require('../../lib/config/config-validator')

describe('Config validator', () => {
  it('should check validSeverityMap', () => {
    assert.deepStrictEqual(validSeverityMap, ['error', 'warn'])
  })

  it('should check defaultSchemaValueForRules', () => {
    assert.deepStrictEqual(defaultSchemaValueForRules, {
      oneOf: [{ type: 'string', enum: ['error', 'warn', 'off'] }, { const: false }],
    })
  })

  it('should validate config', () => {
    const config = {
      extends: [],
      rules: {
        'avoid-throw': 'off',
        indent: ['error', 2],
      },
    }
    assert.deepStrictEqual(_.isUndefined(validate(config)), true)
  })

  it('should throw an error with wrong config', () => {
    const config = {
      test: [],
      rules: {
        'avoid-throw': 'off',
        indent: ['error', 2],
      },
    }
    assert.throws(() => validate(config), Error)
  })

  it('should work with an empty config', () => {
    const config = {}

    validate(config) // should not throw
  })
})
