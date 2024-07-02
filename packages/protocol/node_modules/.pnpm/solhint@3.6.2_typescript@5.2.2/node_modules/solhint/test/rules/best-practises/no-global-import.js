const linter = require('../../../lib/index')
const {
  assertNoWarnings,
  assertLineNumber,
  assertErrorMessage,
  assertWarnsCount,
} = require('../../common/asserts')

describe('Linter - no-global-import', () => {
  it('should raise on import * from "path"', () => {
    const code = `import * from './A.sol';`

    const report = linter.processStr(code, {
      rules: { 'no-global-import': 'warn' },
    })
    assertWarnsCount(report, 1)
    assertErrorMessage(report, 'global import')
    assertErrorMessage(report, 'Specify names to import individually')
  })
  it('should raise on import "path"', () => {
    const code = `import './A.sol';`

    const report = linter.processStr(code, {
      rules: { 'no-global-import': 'warn' },
    })
    assertWarnsCount(report, 1)
    assertErrorMessage(report, 'global import')
    assertErrorMessage(report, 'Specify names to import individually')
  })
  it('should raise warning when using solhint:recommended', () => {
    const code = `pragma solidity ^0.8.0; import "./A.sol";`

    const report = linter.processStr(code, {
      extends: 'solhint:recommended',
    })
    assertWarnsCount(report, 1)
    assertErrorMessage(report, 'global import')
    assertErrorMessage(report, 'Specify names to import individually')
  })
  it('should report correct line', () => {
    const code = `import {A} from './A.sol';
    import './A.sol';`

    const report = linter.processStr(code, {
      rules: { 'no-global-import': 'warn' },
    })
    assertLineNumber(report.reports[0], 2)
    assertWarnsCount(report, 1)
  })
  it('should not raise on import {identifier} from "path"', () => {
    const code = `import {A} from './A.sol';`

    const report = linter.processStr(code, {
      rules: { 'no-global-import': 'warn' },
    })
    assertNoWarnings(report)
  })
  it('should not raise on import from "path" as NAME', () => {
    const code = `import "../../src/Helpers.sol" as H;`

    const report = linter.processStr(code, {
      rules: { 'no-global-import': 'warn' },
    })
    assertNoWarnings(report)
  })
  it('should not raise on import * as NAME from "path" as NAME', () => {
    const code = `import * as H from "../../src/Helpers.sol";`

    const report = linter.processStr(code, {
      rules: { 'no-global-import': 'warn' },
    })
    assertNoWarnings(report)
  })
})
