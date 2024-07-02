'use strict'

module.exports = class ModuleError extends Error {
  /**
   * @param {string} message Error message
   * @param {{ code?: string, cause?: Error, expected?: boolean, transient?: boolean }} [options]
   */
  constructor (message, options) {
    super(message || '')

    if (typeof options === 'object' && options !== null) {
      if (options.code) this.code = String(options.code)
      if (options.expected) this.expected = true
      if (options.transient) this.transient = true
      if (options.cause) this.cause = options.cause
    }

    if (Error.captureStackTrace) {
      Error.captureStackTrace(this, this.constructor)
    }
  }
}
