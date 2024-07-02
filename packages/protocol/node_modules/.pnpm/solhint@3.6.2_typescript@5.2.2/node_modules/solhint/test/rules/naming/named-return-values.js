const assert = require('assert')
const linter = require('../../../lib/index')
const contractWith = require('../../common/contract-builder').contractWith
const { assertErrorCount, assertNoErrors, assertWarnsCount } = require('../../common/asserts')

describe('Linter - named-return-values', () => {
  it('should NOT raise error for named return values', () => {
    const code = contractWith(
      `function getBalanceFromTokens(address wallet) public returns(address token1, address token2, uint256 balance1, uint256 balance2) { balance = 1; }`
    )
    const report = linter.processStr(code, {
      rules: { 'named-return-values': 'error' },
    })
    assertNoErrors(report)
  })

  it('should raise error for unnamed return values', () => {
    const code = contractWith(
      `function getBalanceFromTokens(address wallet) public returns(address, address, uint256, uint256) { balance = 1; }`
    )
    const report = linter.processStr(code, {
      rules: { 'named-return-values': 'error' },
    })

    assertErrorCount(report, 4)
    for (let index = 0; index < report.reports.length; index++) {
      assert.equal(report.reports[index].message, `Named return value is missing - Index ${index}`)
    }
  })

  it('should NOT raise error for functions without return values', () => {
    const code = contractWith(`function writeOnStorage(address wallet) public { balance = 1; }`)
    const report = linter.processStr(code, {
      rules: { 'named-return-values': 'error' },
    })
    assertNoErrors(report)
  })

  it('should raise error for 2 unnamed return values', () => {
    const code = contractWith(
      `function getBalanceFromTokens(address wallet) public returns(address user, address, uint256 amount, uint256) { balance = 1; }`
    )
    const report = linter.processStr(code, {
      rules: { 'named-return-values': 'error' },
    })

    assertErrorCount(report, 2)
    assert.equal(report.reports[0].message, `Named return value is missing - Index 1`)
    assert.equal(report.reports[1].message, `Named return value is missing - Index 3`)
  })

  it('should NOT raise error for solhint:recommended setup', () => {
    const code = contractWith(
      `function getBalanceFromTokens(address wallet) public returns(address, address, uint256, uint256) { balance = 1; }`
    )

    const report = linter.processStr(code, {
      extends: 'solhint:recommended',
      rules: { 'compiler-version': 'off' },
    })

    assertNoErrors(report)
  })

  it('should NOT raise error for solhint:default setup', () => {
    const code = contractWith(
      `function getBalance(address wallet) public returns(uint256) { balance = 1; }`
    )

    const report = linter.processStr(code, {
      extends: 'solhint:default',
    })

    assertNoErrors(report)
  })

  it('should raise error for solhint:all setup', () => {
    const code = contractWith(
      `function getBalance(uint256 wallet) public override returns(uint256, address) { wallet = 1; }`
    )

    const report = linter.processStr(code, {
      extends: 'solhint:all',
      rules: { 'compiler-version': 'off' },
    })

    assertWarnsCount(report, 2)
    for (let index = 0; index < report.reports.length; index++) {
      assert.equal(report.reports[index].message, `Named return value is missing - Index ${index}`)
    }
  })
})
