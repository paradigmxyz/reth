const assert = require('assert')
const {
  assertNoWarnings,
  assertNoErrors,
  assertErrorMessage,
  assertWarnsCount,
  assertErrorCount,
} = require('../../common/asserts')
const linter = require('../../../lib/index')
const { funcWith } = require('../../common/contract-builder')

describe('Linter - reason-string', () => {
  it('should raise reason string is mandatory for require', () => {
    const code = require('../../fixtures/best-practises/require-without-reason')

    const report = linter.processStr(code, {
      rules: { 'reason-string': ['warn', { maxLength: 5 }] },
    })

    assert.ok(report.warningCount > 0)
    assertWarnsCount(report, 1)
    assertErrorMessage(report, 'Provide an error message for require')
  })

  it('should raise reason string is mandatory for revert', () => {
    const code = funcWith(`revert();
        role.bearer[account] = true;role.bearer[account] = true;
    `)

    const report = linter.processStr(code, {
      rules: { 'reason-string': ['error', { maxLength: 5 }] },
    })

    assert.ok(report.errorCount > 0)
    assertErrorCount(report, 1)
    assertErrorMessage(report, 'Provide an error message for revert')
  })

  it('should raise reason string maxLength error for require', () => {
    const code = funcWith(`require(!has(role, account), "Roles: account already has role");
        role.bearer[account] = true;role.bearer[account] = true;
    `)

    const report = linter.processStr(code, {
      rules: { 'reason-string': ['warn', { maxLength: 5 }] },
    })

    assert.ok(report.warningCount > 0)
    assertWarnsCount(report, 1)
    assertErrorMessage(report, 'Error message for require is too long')
  })

  it('should raise reason string maxLength error for revert', () => {
    const code = funcWith(`revert("Roles: account already has role");
        role.bearer[account] = true;role.bearer[account] = true;
    `)

    const report = linter.processStr(code, {
      rules: { 'reason-string': ['error', { maxLength: 5 }] },
    })

    assert.ok(report.errorCount > 0)
    assertErrorCount(report, 1)
    assertErrorMessage(report, 'Error message for revert is too long')
  })

  it('should not raise warning for require', () => {
    const code = require('../../fixtures/best-practises/require-with-reason')

    const report = linter.processStr(code, {
      rules: { 'reason-string': ['warn', { maxLength: 31 }] },
    })

    assertNoWarnings(report)
    assertNoErrors(report)
  })

  it('should not raise error for revert', () => {
    const code = funcWith(`revert("Roles: account already has role");
        role.bearer[account] = true;role.bearer[account] = true;
    `)

    const report = linter.processStr(code, {
      rules: { 'reason-string': ['error', { maxLength: 50 }] },
    })

    assertNoWarnings(report)
    assertNoErrors(report)
  })

  it('should ignore normal function calls', () => {
    const code = funcWith(`
      revertt();
      requiree(true);
    `)

    const report = linter.processStr(code, {
      rules: { 'reason-string': ['error', { maxLength: 50 }] },
    })

    assertNoWarnings(report)
    assertNoErrors(report)
  })

  it('should raise reason string maxLength error with added data', () => {
    const qtyChars = 'Roles: account already has role'.length
    const maxLength = 5

    const code = funcWith(`require(!has(role, account), "Roles: account already has role");
        role.bearer[account] = true;role.bearer[account] = true;
    `)

    const report = linter.processStr(code, {
      rules: { 'reason-string': ['warn', { maxLength: 5 }] },
    })

    assert.ok(report.warningCount > 0)
    assertWarnsCount(report, 1)

    assert.equal(
      report.reports[0].message,
      `Error message for require is too long: ${qtyChars} counted / ${maxLength} allowed`
    )
  })
})
