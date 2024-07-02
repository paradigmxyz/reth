const BaseChecker = require('../base-checker')

class BaseDeprecation extends BaseChecker {
  constructor(reporter, ruleId, meta) {
    super(reporter, ruleId, meta)
    this.active = false
    this.deprecationVersion() // to ensure we have one.
  }

  PragmaDirective(node) {
    const pragma = node.name
    const value = node.value
    if (pragma === 'solidity') {
      const contextVersion = value.replace(/[^0-9.]/g, '').split('.')
      const deprecationAt = this.deprecationVersion().split('.')
      this.active =
        contextVersion[0] > deprecationAt[0] ||
        contextVersion[1] > deprecationAt[1] ||
        contextVersion[2] >= deprecationAt[2]
      this.version = value
    }
  }

  deprecationVersion() {
    throw new Error('Implementations must supply a deprecation version!')
  }
}

module.exports = BaseDeprecation
