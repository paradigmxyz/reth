const BaseChecker = require('../base-checker')
const { severityDescription } = require('../../doc/utils')

const lineBreakPattern = /\r\n|[\r\n\u2028\u2029]/u

const ruleId = 'max-line-length'
const DEFAULT_SEVERITY = 'error'
const DEFAULT_MAX_LINE_LENGTH = 120
const meta = {
  type: 'best-practises',

  docs: {
    description: 'Line length must be no more than maxlen.',
    category: 'Best Practise Rules',
    options: [
      {
        description: severityDescription,
        default: DEFAULT_SEVERITY,
      },
      {
        description: 'Maximum allowed number of characters per line',
        default: DEFAULT_MAX_LINE_LENGTH,
      },
    ],
  },

  isDefault: true,
  recommended: false,
  defaultSetup: [DEFAULT_SEVERITY, DEFAULT_MAX_LINE_LENGTH],

  schema: { type: 'integer', minimum: 1 },
}

class MaxLineLengthChecker extends BaseChecker {
  constructor(reporter, config, inputSrc) {
    super(reporter, ruleId, meta)

    this.maxLength = (config && config.getNumber(ruleId, 120)) || 120
    this.inputSrc = inputSrc
  }

  SourceUnit() {
    const lines = this.inputSrc.split(lineBreakPattern)
    lines.map((line) => line.length).forEach(this.validateLineLength.bind(this))
  }

  validateLineLength(curLength, lineNum) {
    if (curLength > this.maxLength) {
      const message = `Line length must be no more than ${this.maxLength} but current length is ${curLength}.`
      this.errorAt(lineNum + 1, 1, message)
    }
  }
}

module.exports = MaxLineLengthChecker
