const assert = require('assert')

function assertErrorMessage(...args) {
  const report = args[0]
  let index
  let message

  if (args.length === 3) {
    index = args[1]
    message = args[2]
  } else {
    index = 0
    message = args[1]
  }

  const errorMessage = `Report should contain message with text "${message}" at ${index} pos. ${printReport(
    report
  )}`
  assert.ok(report.messages[index].message.includes(message), errorMessage)
}

function assertNoErrors(report) {
  assert.equal(report.errorCount, 0, `Report must not contain errors. ${printReport(report)}`)
}

function assertNoWarnings(report) {
  assert.equal(report.warningCount, 0, `Report must not contain warnings. ${printReport(report)}`)
}

function assertErrorCount(report, count) {
  assert.equal(
    report.errorCount,
    count,
    `Report must contains ${count} errors. ${printReport(report)}`
  )
}

function assertWarnsCount(report, count) {
  assert.equal(
    report.warningCount,
    count,
    `Report must contains ${count} warnings. ${printReport(report)}`
  )
}

function assertLineNumber(report, line) {
  assert.equal(report.line, line, `Report must be in line ${line}.`)
}

function printReport(report) {
  const messages = report.messages.map((i, index) => `${index + 1}. ${i.message}`)
  return ['Errors / Warnings:', ...messages, ''].join('\n' + ' '.repeat(8))
}

module.exports = {
  assertErrorMessage,
  assertNoWarnings,
  assertNoErrors,
  assertErrorCount,
  assertWarnsCount,
  assertLineNumber,
}
