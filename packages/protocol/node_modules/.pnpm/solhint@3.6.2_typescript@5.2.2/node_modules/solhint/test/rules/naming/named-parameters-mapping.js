const assert = require('assert')
const { assertWarnsCount } = require('../../common/asserts')
const linter = require('../../../lib/index')
const contractWith = require('../../common/contract-builder').contractWith
const {
  NAMED_MAPPING_REGULAR,
  NAMED_MAPPING_NESTED,
  OTHER_OK_DECLARATIONS,
  NO_NAMED_MAPPING_REGULAR,
  NO_NAMED_MAPPING_NESTED,
  OTHER_WRONG_DECLARATIONS,
} = require('../../fixtures/naming/named-parameters-mapping')

const WRONG_DECLARATIONS = NO_NAMED_MAPPING_REGULAR.concat(
  NO_NAMED_MAPPING_NESTED,
  OTHER_WRONG_DECLARATIONS
)
const MAIN_KEY_ERROR = 'Main key parameter in mapping XXXXX is not named'
const VALUE_ERROR = 'Value parameter in mapping XXXXX is not named'

const getPositionErrors = (objectCode) => {
  const errorArray = []
  if (objectCode.error_mainKey)
    errorArray.push(MAIN_KEY_ERROR.replace('XXXXX', objectCode.mapping_name))

  if (objectCode.error_value) errorArray.push(VALUE_ERROR.replace('XXXXX', objectCode.mapping_name))
  return errorArray
}

describe('Linter - Named parameters for mapping', () => {
  it('should not raise an error if all parameters are named correctly for regular mapping', () => {
    const code = contractWith(NAMED_MAPPING_REGULAR)
    const report = linter.processStr(code, {
      rules: { 'named-parameters-mapping': 'error' },
    })

    assert.equal(report.errorCount, 0)
  })

  it('should not raise an error if all parameters are named correctly for nested mapping', () => {
    const code = contractWith(NAMED_MAPPING_NESTED)
    const report = linter.processStr(code, {
      rules: { 'named-parameters-mapping': 'error' },
    })

    assert.equal(report.errorCount, 0)
  })

  OTHER_OK_DECLARATIONS.forEach((objectCode) =>
    it('should not raise an error if all parameters are named correctly for correct declarations', () => {
      const code = contractWith(objectCode.code)
      const report = linter.processStr(code, {
        rules: { 'named-parameters-mapping': 'error' },
      })

      assert.equal(report.errorCount, 0)
    })
  )

  WRONG_DECLARATIONS.forEach((objectCode) => {
    it(`should raise an error if all parameters are not named for nested mappings`, () => {
      const code = contractWith(objectCode.code)
      const report = linter.processStr(code, {
        rules: { 'named-parameters-mapping': 'error' },
      })

      const errorPositions = getPositionErrors(objectCode)
      const qtyErrors = errorPositions.length

      assert.equal(report.errorCount, qtyErrors)

      for (let i = 0; i < errorPositions.length; i++) {
        assert.equal(report.reports[i].message, errorPositions[i])
      }
    })
  })

  // WARNING FLAG
  WRONG_DECLARATIONS.forEach((objectCode) =>
    it('should raise a warning if all parameters are not named for nested mappings', () => {
      const code = contractWith(objectCode.code)
      const report = linter.processStr(code, {
        rules: { 'named-parameters-mapping': 'warn' },
      })

      const errorPositions = getPositionErrors(objectCode)
      const qtyErrors = errorPositions.length

      assert.equal(report.errorCount, 0)
      assertWarnsCount(report, qtyErrors)

      for (let i = 0; i < errorPositions.length; i++) {
        assert.equal(report.reports[i].message, errorPositions[i])
      }
    })
  )

  // WARNING FLAG
  OTHER_OK_DECLARATIONS.forEach((objectCode) =>
    it('should not raise an error if all parameters are named correctly for correct declarations', () => {
      const code = contractWith(objectCode.code)
      const report = linter.processStr(code, {
        rules: { 'named-parameters-mapping': 'warn' },
      })

      assert.equal(report.errorCount, 0)
      assertWarnsCount(report, 0)
    })
  )

  // OFF FLAG
  WRONG_DECLARATIONS.forEach((objectCode) =>
    it('should raise a warning if all parameters are not named for nested mappings', () => {
      const code = contractWith(objectCode.code)
      const report = linter.processStr(code, {
        rules: { 'named-parameters-mapping': 'off' },
      })

      assert.equal(report.errorCount, 0)
      assertWarnsCount(report, 0)
    })
  )

  // OFF FLAG
  OTHER_OK_DECLARATIONS.forEach((objectCode) =>
    it('should not raise an error if all parameters are named correctly for correct declarations', () => {
      const code = contractWith(objectCode.code)
      const report = linter.processStr(code, {
        rules: { 'named-parameters-mapping': 'off' },
      })

      assert.equal(report.errorCount, 0)
      assertWarnsCount(report, 0)
    })
  )
})
