const assert = require('assert')

const applyFixes = require('../../lib/apply-fixes')

describe('applyFixes', () => {
  it('should work when there are no reports', () => {
    const inputSrc = 'contract Foo {}'
    const fixes = []

    const { fixed } = applyFixes(fixes, inputSrc)

    assert.equal(fixed, false)
  })

  it('should work for a single replace', () => {
    const inputSrc = `
contract Foo {
  function foo() {
    throw;
  }
}`.trim()

    const fixes = [
      {
        range: [38, 42],
        text: 'revert()',
      },
    ]

    const { fixed, output } = applyFixes(fixes, inputSrc)

    assert.equal(fixed, true)
    assert.equal(
      output,
      `
contract Foo {
  function foo() {
    revert();
  }
}`.trim()
    )
  })

  it('should work for two fixes', () => {
    const inputSrc = `
contract Foo {
  function foo() {
    throw;
    throw;
  }
}`.trim()

    const fixes = [
      {
        range: [38, 42],
        text: 'revert()',
      },
      {
        range: [49, 53],
        text: 'revert()',
      },
    ]

    const { fixed, output } = applyFixes(fixes, inputSrc)

    assert.equal(fixed, true)
    assert.equal(
      output,
      `
contract Foo {
  function foo() {
    revert();
    revert();
  }
}`.trim()
    )
  })
})
