const assert = require('assert')
const linter = require('../../../lib/index')
const { funcWith, contractWith } = require('../../common/contract-builder')

describe('Linter - mark-callable-contracts', () => {
  it('should return error that external contract is not marked as trusted / untrusted', () => {
    const code = funcWith(require('../../fixtures/security/external-contract-untrusted'))

    const report = linter.processStr(code, {
      rules: { 'mark-callable-contracts': 'warn' },
    })

    assert.equal(report.warningCount, 1)
    assert.ok(report.reports[0].message.includes('trusted'))
  })

  it('should not return error for external contract that is marked as trusted', () => {
    const code = funcWith(require('../../fixtures/security/external-contract-trusted'))

    const report = linter.processStr(code, {
      rules: { 'mark-callable-contracts': 'warn' },
    })

    assert.equal(report.warningCount, 0)
  })

  it('should not return error for external contract that is marked as untrusted', () => {
    const code = funcWith('UntrustedBank.withdraw(100);')

    const report = linter.processStr(code, {
      rules: { 'mark-callable-contracts': 'warn' },
    })

    assert.equal(report.warningCount, 0)
  })

  it('should not return error for a struct', () => {
    const code = contractWith(`
  struct Token {
    address tokenAddress;
    string tokenSymbol;
  }

  mapping(address => Token) public acceptedTokens;

  function b(address tokenAddress, string memory tokenSymbol) public {
    Token memory token = Token(tokenAddress, tokenSymbol);
    acceptedTokens[tokenAddress] = token;
  }
    `)

    const report = linter.processStr(code, {
      rules: { 'mark-callable-contracts': 'warn' },
    })

    assert.equal(report.warningCount, 0)
  })

  it('should not return error for an event', () => {
    const code = contractWith(`
  event UpdatedToken();

  function b() public {
    emit UpdatedToken();
  }
    `)

    const report = linter.processStr(code, {
      rules: { 'mark-callable-contracts': 'warn' },
    })

    assert.equal(report.warningCount, 0)
  })

  it('should not return error for an event defined after function', () => {
    const code = contractWith(`
      function b() public {
        emit UpdatedToken();
      }

      event UpdatedToken();
    `)

    const report = linter.processStr(code, {
      rules: { 'mark-callable-contracts': 'warn' },
    })

    assert.equal(report.warningCount, 0)
  })

  it('should not return error for constant', () => {
    const code = contractWith(`
  uint8 private constant TOKEN_DECIMALS = 15;

  function b() public view returns(uint8) {
    return TOKEN_DECIMALS;
  }
    `)

    const report = linter.processStr(code, {
      rules: { 'mark-callable-contracts': 'warn' },
    })

    assert.equal(report.warningCount, 0)
  })

  it('should not return error for enum', () => {
    const code = contractWith(`
  enum Status { Initial }

  function b() public view returns(Status) {
    return Status.Initial;
  }
    `)

    const report = linter.processStr(code, {
      rules: { 'mark-callable-contracts': 'warn' },
    })

    assert.equal(report.warningCount, 0)
  })
  it('should not return error for an event defined after function', () => {
    const code = contractWith(`
      function b() public {
        emit UpdatedToken();
      }

      event UpdatedToken();
    `)

    const report = linter.processStr(code, {
      rules: { 'mark-callable-contracts': 'warn' },
    })

    assert.equal(report.warningCount, 0)
  })
})
