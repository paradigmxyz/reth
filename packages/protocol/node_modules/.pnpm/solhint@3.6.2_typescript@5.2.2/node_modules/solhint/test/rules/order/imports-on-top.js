const assert = require('assert')
const linter = require('../../../lib/index')

describe('Linter - imports-on-top', () => {
  it('should raise import not on top error', () => {
    const code = `
              contract A {}


              import "lib.sol";
            `

    const report = linter.processStr(code, {
      rules: { 'imports-on-top': 'error' },
    })

    assert.equal(report.errorCount, 1)
    assert.ok(report.messages[0].message.includes('Import'))
  })

  it('should not raise import not on top error', () => {
    const code = `
                pragma solidity 0.4.17;
                import "lib.sol";


                contract A {}
            `

    const report = linter.processStr(code, {
      rules: { 'imports-on-top': 'error' },
    })

    assert.equal(report.errorCount, 0)
  })
})
