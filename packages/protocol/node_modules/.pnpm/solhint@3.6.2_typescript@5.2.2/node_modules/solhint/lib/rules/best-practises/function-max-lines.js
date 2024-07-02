const BaseChecker = require('../base-checker')
const { severityDescription } = require('../../doc/utils')

const DEFAULT_SEVERITY = 'warn'
const DEFAULT_MAX_LINES_COUNT = 50
const ruleId = 'function-max-lines'
const meta = {
  type: 'best-practises',

  docs: {
    description: 'Function body contains "count" lines but allowed no more than maxlines.',
    category: 'Best Practise Rules',
    options: [
      {
        description: severityDescription,
        default: DEFAULT_SEVERITY,
      },
      {
        description: 'Maximum allowed lines count per function',
        default: DEFAULT_MAX_LINES_COUNT,
      },
    ],
  },

  isDefault: false,
  recommended: false,
  defaultSetup: [DEFAULT_SEVERITY, DEFAULT_MAX_LINES_COUNT],

  schema: {
    type: 'integer',
    minimum: 1,
  },
}

class FunctionMaxLinesChecker extends BaseChecker {
  constructor(reporter, config) {
    super(reporter, ruleId, meta)

    this.maxLines =
      (config && config.getNumber(ruleId, DEFAULT_MAX_LINES_COUNT)) || DEFAULT_MAX_LINES_COUNT
  }

  FunctionDefinition(node) {
    if (this._linesCount(node) > this.maxLines) {
      this._error(node)
    }
  }

  _linesCount(node) {
    const startStopGap = node.loc.end.line - node.loc.start.line

    if (this._isSingleLineBlock(startStopGap)) {
      return 1
    } else {
      return this._withoutCloseBracket(startStopGap)
    }
  }

  _isSingleLineBlock(startStopGap) {
    return startStopGap === 0
  }

  _withoutCloseBracket(startStopGap) {
    return startStopGap - 1
  }

  _error(node) {
    const linesCount = this._linesCount(node)
    const message = `Function body contains ${linesCount} lines but allowed no more than ${this.maxLines} lines`
    this.error(node, message)
  }
}

module.exports = FunctionMaxLinesChecker
