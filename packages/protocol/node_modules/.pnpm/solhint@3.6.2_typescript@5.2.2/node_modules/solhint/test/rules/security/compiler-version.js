const assert = require('assert')
const { assertNoErrors, assertErrorCount, assertErrorMessage } = require('../../common/asserts')
const linter = require('../../../lib/index')

const DEFAULT_SEMVER = '^0.8.0'

describe('Linter - compiler-version', () => {
  it('should disable only one compiler error on next line', () => {
    const report = linter.processStr(
      `
                // solhint-disable-next-line
                pragma solidity ^0.4.4;
                pragma solidity 0.3.4;
            `,
      {
        rules: { 'compiler-version': ['error', DEFAULT_SEMVER] },
      }
    )

    assertErrorCount(report, 1)
  })

  it('should disable only one compiler error on previous line', () => {
    const report = linter.processStr(
      `
              pragma solidity 0.3.4;
              // solhint-disable-previous-line
              pragma solidity 0.3.4;
          `,
      {
        rules: { 'compiler-version': ['error', DEFAULT_SEMVER] },
      }
    )

    assertErrorCount(report, 1)
  })

  it('should disable only one compiler error on next line using multiline comment', () => {
    const report = linter.processStr(
      `
                /* solhint-disable-next-line */
                pragma solidity ^0.4.4;
                pragma solidity 0.3.4;
            `,
      {
        rules: { 'compiler-version': ['error', DEFAULT_SEMVER] },
      }
    )

    assertErrorCount(report, 1)
  })

  it('should disable only one compiler error on previous line using multiline comment', () => {
    const report = linter.processStr(
      `
                pragma solidity ^0.4.4;
                /* solhint-disable-previous-line */
                pragma solidity 0.3.4;
            `,
      {
        rules: { 'compiler-version': ['error', DEFAULT_SEMVER] },
      }
    )

    assertErrorCount(report, 1)
  })

  it('should disable only one compiler version error', () => {
    const report = linter.processStr(
      `
                /* solhint-disable compiler-version */
                pragma solidity 0.3.4;
                /* solhint-enable compiler-version */
                pragma solidity 0.3.4;
            `,
      {
        rules: { 'compiler-version': ['error', DEFAULT_SEMVER] },
      }
    )

    assertErrorCount(report, 1)
    assertErrorMessage(report, DEFAULT_SEMVER)
  })

  it('should disable all errors', () => {
    const report = linter.processStr(
      `
                /* solhint-disable */
                pragma solidity ^0.4.4;
                pragma solidity 0.3.4;
            `,
      {
        rules: { 'compiler-version': ['error', DEFAULT_SEMVER] },
      }
    )
    assertNoErrors(report)
  })

  it('should disable then enable all errors', () => {
    const report = linter.processStr(
      `
                /* solhint-disable */
                pragma solidity ^0.4.4;
                /* solhint-enable */
                pragma solidity ^0.4.4;
            `,
      {
        rules: { 'compiler-version': ['error', DEFAULT_SEMVER] },
      }
    )

    assertErrorCount(report, 1)
    assertErrorMessage(report, DEFAULT_SEMVER)
  })

  it('should return compiler version error', () => {
    const report = linter.processStr('pragma solidity 0.3.4;', {
      rules: { 'compiler-version': ['error', DEFAULT_SEMVER] },
    })

    assert.equal(report.errorCount, 1)
    assert.ok(report.reports[0].message.includes(DEFAULT_SEMVER))
  })

  it('should not report compiler version error on exact match', () => {
    const report = linter.processStr('pragma solidity 0.8.0;', {
      rules: { 'compiler-version': ['error', '0.8.0'] },
    })

    assert.equal(report.errorCount, 0)
  })

  it('should not report compiler version error on range match', () => {
    const report = linter.processStr('pragma solidity ^0.8.0;', {
      rules: { 'compiler-version': ['error', DEFAULT_SEMVER] },
    })

    assert.equal(report.errorCount, 0)
  })

  it('should not report compiler version error on patch bump', () => {
    const report = linter.processStr('pragma solidity 0.8.1;', {
      rules: { 'compiler-version': ['error', DEFAULT_SEMVER] },
    })

    assert.equal(report.errorCount, 0)
  })

  it('should not report compiler version error on range match', () => {
    const report = linter.processStr('pragma solidity ^0.8.2;', {
      rules: { 'compiler-version': ['error', DEFAULT_SEMVER] },
    })

    assert.equal(report.errorCount, 0)
  })

  it('should report compiler version error on range not matching', () => {
    const report = linter.processStr('pragma solidity ^0.8.1;', {
      rules: { 'compiler-version': ['error', '^0.8.3'] },
    })

    assert.equal(report.errorCount, 1)
  })

  it('should report compiler version error on minor bump', () => {
    const report = linter.processStr('pragma solidity 0.6.0;', {
      rules: { 'compiler-version': ['error', DEFAULT_SEMVER] },
    })

    assert.equal(report.errorCount, 1)
  })

  it(`should report compiler version error if pragma doesn't exist`, () => {
    const report = linter.processStr('contract Foo {}', {
      rules: { 'compiler-version': ['error', DEFAULT_SEMVER] },
    })

    assert.equal(report.errorCount, 1)
  })

  it(`should not report compiler version error if pragma exist`, () => {
    const report = linter.processStr(
      `pragma solidity 0.8.2;
      contract Foo {}`,
      {
        rules: { 'compiler-version': ['error', '^0.8.2'] },
      }
    )

    assert.equal(report.errorCount, 0)
  })

  it(`should not report compiler version error with correct pragma and pragma experimental`, () => {
    const report = linter.processStr(
      `pragma solidity ^0.7.4;
       pragma experimental ABIEncoderV2;
       
       contract Main {
       }`,
      {
        rules: { 'compiler-version': ['error', '^0.7.4'] },
      }
    )

    assert.equal(report.errorCount, 0)
  })

  it(`should not report compiler version error using default and correct pragma`, () => {
    const report = linter.processStr(
      `pragma solidity ^0.8.1;
       pragma experimental ABIEncoderV2;
       
       contract Main {
       }`,
      {
        rules: { 'compiler-version': 'error' },
      }
    )

    assert.equal(report.errorCount, 0)
  })

  it(`should report compiler version error using default and lower pragma`, () => {
    const report = linter.processStr(
      `pragma solidity ^0.7.4;
       
       contract Main {
       }`,
      {
        rules: { 'compiler-version': 'error' },
      }
    )

    assert.equal(report.errorCount, 1)
    assertErrorMessage(report, DEFAULT_SEMVER)
  })

  it(`should not report compiler version error using >= and default and correct pragma`, () => {
    const report = linter.processStr(
      `pragma solidity >=0.8.0;
       
       contract Main {
       }`,
      {
        rules: { 'compiler-version': 'error' },
      }
    )

    assert.equal(report.errorCount, 0)
  })

  it(`should report compiler version error using >= and default and lower pragma`, () => {
    const report = linter.processStr(
      `pragma solidity >=0.7.4;
       
       contract Main {
       }`,
      {
        rules: { 'compiler-version': 'error' },
      }
    )
    assert.equal(report.errorCount, 1)
    assertErrorMessage(report, DEFAULT_SEMVER)
  })
})
