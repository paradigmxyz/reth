const { assertNoWarnings, assertErrorMessage, assertWarnsCount } = require('../../common/asserts')
const linter = require('../../../lib/index')
const { contractWith, funcWith } = require('../../common/contract-builder')

describe('Linter - no-empty-blocks', () => {
  const EMPTY_BLOCKS = [
    funcWith('if (a < b) {  }'),
    contractWith('struct Abc {  }'),
    contractWith('enum Abc {  }'),
    contractWith('constructor () {  }'),
    'contract A { }',
    funcWith('assembly {  }'),
  ]

  EMPTY_BLOCKS.forEach((curData) =>
    it(`should raise warn for empty blocks ${label(curData)}`, () => {
      const report = linter.processStr(curData, {
        rules: { 'no-empty-blocks': 'warn' },
      })

      assertWarnsCount(report, 1)
      assertErrorMessage(report, 'empty block')
    })
  )

  const BLOCKS_WITH_DEFINITIONS = [
    contractWith('function () public payable { make1(); }'),
    contractWith('receive() external payable {}'),
    contractWith('fallback() external payable {}'),
    funcWith('if (a < b) { make1(); }'),
    contractWith('struct Abc { uint a; }'),
    contractWith('enum Abc { Test1 }'),
    'contract A { uint private a; }',
    funcWith('assembly { "literal" }'),
    contractWith('constructor () BaseContract() {  }'),
  ]

  BLOCKS_WITH_DEFINITIONS.forEach((curData) =>
    it(`should not raise warn for blocks ${label(curData)}`, () => {
      const report = linter.processStr(curData, {
        rules: { 'no-empty-blocks': 'warn' },
      })

      assertNoWarnings(report)
    })
  )

  it('should not raise error for default function', () => {
    const defaultFunction = contractWith('function () public payable {}')
    const report = linter.processStr(defaultFunction, {
      rules: { 'no-empty-blocks': 'warn' },
    })

    assertNoWarnings(report)
  })

  it('should not raise error for inline assembly [for] statement with some content', () => {
    const code = funcWith(`
      assembly {  
        for { } lt(i, 0x100) { } {     
          i := add(i, 0x20)
        } 
      }`)

    const report = linter.processStr(code, {
      rules: { 'no-empty-blocks': 'warn' },
    })

    assertNoWarnings(report)
  })

  it('should raise error for inline assembly [for] statement with empty content', () => {
    const code = funcWith(`
      assembly {  
        for { } lt(i, 0x100) { } {     
        } 
      }`)

    const report = linter.processStr(code, {
      rules: { 'no-empty-blocks': 'warn' },
    })
    assertWarnsCount(report, 1)
    assertErrorMessage(report, 'empty block')
  })

  it('should raise error for inline assembly [for nested] statement with empty content', () => {
    const code = funcWith(`
      assembly {  
        for { } lt(i, 0x100) { } {     
          for { } lt(j, 0x100) { } {     
          } 
        } 
      }`)

    const report = linter.processStr(code, {
      rules: { 'no-empty-blocks': 'warn' },
    })
    assertWarnsCount(report, 1)
    assertErrorMessage(report, 'empty block')
  })

  it('should not raise error for inline assembly [for nested] statement with some content', () => {
    const code = funcWith(`
      assembly {  
        for { } lt(i, 0x100) { } {
          i := add(i, 0x20)     
          for { } lt(i, 0x100) { } {     
            j := add(j, 0x20)
          } 
        } 
      }`)

    const report = linter.processStr(code, {
      rules: { 'no-empty-blocks': 'warn' },
    })

    assertNoWarnings(report)
  })

  function label(data) {
    const items = data.split('\n')
    const lastItemIndex = items.length - 1
    const labelIndex = Math.floor(lastItemIndex / 5) * 4
    return items[labelIndex]
  }
})
