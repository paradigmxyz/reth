const assert = require('assert')
const { assertErrorCount } = require('./common/asserts')
const linter = require('../lib/index')

describe('Parse error', () => {
  it('should report parse errors', () => {
    const report = linter.processStr('contract Foo {', {})

    assertErrorCount(report, 1)
    const error = report.reports[0]
    assert.equal(error.line, 1)
    assert.equal(error.column, 14)
    assert.ok(error.message.startsWith("Parse error: mismatched input '<EOF>'"))
  })

  it('should report multiple parse errors', () => {
    const report = linter.processStr(
      `
 contract Foo {}}
 contract Bar {
 `,
      {}
    )

    assertErrorCount(report, 2)
    const messages = report.reports.map((error) => error.message)
    assert.ok(messages[0].startsWith("Parse error: extraneous input '}' expecting"))
    assert.ok(messages[1].startsWith("Parse error: mismatched input '<EOF>' expecting"))
  })
})
