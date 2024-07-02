class CommentDirectiveParser {
  constructor(tokens) {
    const lastToken = tokens[tokens.length - 1]
    this.lastLine = lastToken ? lastToken.loc.end.line : 0
    this.ruleStore = new RuleStore(this.lastLine)

    this.parseComments(tokens)
  }

  parseComments(tokens) {
    const items = tokens.filter(
      (token) => token.type === 'Keyword' && /^(\/\/|\/\*)/.test(token.value)
    )
    items.forEach((item) => this.onComment(item))
  }

  onComment(lexema) {
    const text = lexema.value
    const curLine = lexema.loc.start.line
    const ruleStore = this.ruleStore

    if (text.includes('solhint-disable-line')) {
      const rules = this.parseRuleIds(text, 'solhint-disable-line')
      ruleStore.disableRules(curLine, rules)
      return
    }

    if (text.includes('solhint-disable-next-line')) {
      const rules = this.parseRuleIds(text, 'solhint-disable-next-line')

      if (curLine + 1 <= this.lastLine) {
        ruleStore.disableRules(curLine + 1, rules)
      }

      return
    }

    if (text.includes('solhint-disable-previous-line')) {
      const rules = this.parseRuleIds(text, 'solhint-disable-previous-line')

      if (curLine > 0) {
        ruleStore.disableRules(curLine - 1, rules)
      }

      return
    }

    if (text.includes('solhint-disable')) {
      const rules = this.parseRuleIds(text, 'solhint-disable')

      ruleStore.disableRulesToEndOfFile(curLine, rules)

      return
    }

    if (text.includes('solhint-enable')) {
      const rules = this.parseRuleIds(text, 'solhint-enable')

      ruleStore.enableRulesToEndOfFile(curLine, rules)
    }
  }

  parseRuleIds(text, start) {
    const ruleIds = text.replace('//', '').replace('/*', '').replace('*/', '').replace(start, '')

    const rules = ruleIds
      .split(',')
      .map((curRule) => curRule.trim())
      .filter((i) => i.length > 0)

    return rules.length > 0 ? rules : 'all'
  }

  isRuleEnabled(line, ruleId) {
    return this.ruleStore.isRuleEnabled(line, ruleId)
  }
}

class RuleStore {
  constructor(lastLine) {
    this.disableRuleByLine = []
    this.disableAllByLine = []
    this.lastLine = lastLine

    this.initRulesTable()
  }

  initRulesTable() {
    for (let i = 1; i <= this.lastLine; i += 1) {
      this.disableRuleByLine[i] = new Set()
      this.disableAllByLine[i] = false
    }
  }

  disableRules(curLine, newRules) {
    if (newRules === 'all') {
      this.disableAllByLine[curLine] = true
    } else {
      const lineRules = this.disableRuleByLine[curLine]
      this.disableRuleByLine[curLine] = new Set([...lineRules, ...newRules])
    }
  }

  disableRulesToEndOfFile(startLine, rules) {
    this._toEndOfFile(startLine, (i) => this.disableRules(i, rules))
  }

  enableRules(curLine, rules) {
    if (rules === 'all') {
      this.disableAllByLine[curLine] = false
    } else {
      const lineRules = this.disableRuleByLine[curLine]
      rules.forEach((curRule) => lineRules.delete(curRule))
    }
  }

  enableRulesToEndOfFile(startLine, rules) {
    this._toEndOfFile(startLine, (i) => this.enableRules(i, rules))
  }

  isRuleEnabled(line, ruleId) {
    const allRulesDisabled = this.disableAllByLine[line]
    const ruleDisabled = this.disableRuleByLine[line] && this.disableRuleByLine[line].has(ruleId)
    return !allRulesDisabled && !ruleDisabled
  }

  _toEndOfFile(from, callback) {
    if (!callback) {
      return
    }

    for (let i = from; i <= this.lastLine; i += 1) {
      callback(i)
    }
  }
}

module.exports = CommentDirectiveParser
