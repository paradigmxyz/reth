const BaseChecker = require('../base-checker')

const ruleId = 'avoid-call-value'
const meta = {
  type: 'security',

  docs: {
    description: `Avoid to use ".call.value()()".`,
    category: 'Security Rules',
  },

  isDefault: false,
  recommended: true,
  defaultSetup: 'warn',

  schema: null,
}

class AvoidCallValueChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  MemberAccess(node) {
    this.validateCallValue(node)
  }

  validateCallValue(node) {
    if (node.memberName === 'value' && node.expression.memberName === 'call') {
      this.error(node, 'Avoid to use ".call.value()()"')
    }
  }
}

module.exports = AvoidCallValueChecker
