/**
 * @fileoverview JSON Style formatter
 * @author ArturLukianov <Original Idea and base source code> <https://github.com/ArturLukianov>
 */

//------------------------------------------------------------------------------
// Helper Functions
//------------------------------------------------------------------------------

/**
 * Returns a canonical error level string based upon the error message passed in.
 * @param {Object} message Individual error message provided by eslint
 * @returns {string} Error level string
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
  const allMessages = []
  let errors = 0
  let warnings = 0

  results.forEach((result) => {
    const messages = result.messages

    messages.forEach((message) => {
      const fullObject = { ...message, filePath: result.filePath }
      fullObject.severity = getMessageType(fullObject)
      if (fullObject.severity === 'Error') errors += 1
      else warnings += 1
      allMessages.push(fullObject)
    })
  })

  let finalMessage
  if (errors > 0 && warnings > 0) {
    finalMessage = {
      conclusion: `${errors + warnings} problem/s (${errors} error/s, ${warnings} warning/s)`,
    }
  } else if (errors > 0 && warnings === 0) {
    finalMessage = {
      conclusion: `${errors} problem/s (${errors} error/s)`,
    }
  } else if (errors === 0 && warnings > 0) {
    finalMessage = {
      conclusion: `${warnings} problem/s (${warnings} warning/s)`,
    }
  }

  if (finalMessage) allMessages.push(finalMessage)

  return allMessages.length > 0 ? JSON.stringify(allMessages) : ''
}
