const BaseChecker = require('../base-checker')
const naming = require('../../common/identifier-naming')

const ruleId = 'event-name-camelcase'
const meta = {
  type: 'naming',

  docs: {
    description: 'Event name must be in CamelCase.',
    category: 'Style Guide Rules',
  },

  isDefault: false,
  recommended: true,
  defaultSetup: 'warn',

  schema: null,
}

class EventNameCamelcaseChecker extends BaseChecker {
  constructor(reporter) {
    super(reporter, ruleId, meta)
  }

  EventDefinition(node) {
    if (naming.isNotCamelCase(node.name)) {
      this.error(node, 'Event name must be in CamelCase')
    }
  }
}

module.exports = EventNameCamelcaseChecker
