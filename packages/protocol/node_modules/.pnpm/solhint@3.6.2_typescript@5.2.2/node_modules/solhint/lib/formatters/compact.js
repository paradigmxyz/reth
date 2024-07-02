/**
 * @fileoverview Compact reporter
 * @author Nicholas C. Zakas
 */

//------------------------------------------------------------------------------
// Helper Functions
//------------------------------------------------------------------------------

/**
 * Returns the severity of warning or error
 * @param {Object} message message object to examine
 * @returns {string} severity level
 * @private
 */
function getMessageType(message) {
  if (message.fatal || message.severity === 2) {
    return 'Error'
  }
  return 'Warning'
}

//------------------------------------------------------------------------------
// Public Interface
//------------------------------------------------------------------------------

// eslint-disable-next-line func-names
module.exports = function (results) {
  let output = ''
  let total = 0
  let errors = 0
  let warnings = 0

  results.forEach((result) => {
    const messages = result.messages

    total += messages.length

    messages.forEach((message) => {
      output += `${result.filePath}: `
      output += `line ${message.line || 0}`
      output += `, col ${message.column || 0}`
      output += `, ${getMessageType(message)}`
      output += ` - ${message.message}`
      output += message.ruleId ? ` (${message.ruleId})` : ''
      output += '\n'
      if (message.severity === 2) errors += 1
      else warnings += 1
    })
  })

  let finalMessage = ''
  if (errors > 0 && warnings > 0) {
    finalMessage = `${errors + warnings} problem/s (${errors} error/s, ${warnings} warning/s)`
  } else if (errors > 0 && warnings === 0) {
    finalMessage = `${errors} problem/s (${errors} error/s)`
  } else if (errors === 0 && warnings > 0) {
    finalMessage = `${warnings} problem/s (${warnings} warning/s)`
  }

  if (total > 0) {
    output += `\n${finalMessage}`
  }

  return output
}
