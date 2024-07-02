function match(text, regex) {
  return text.replace(regex, '').length === 0
}

module.exports = {
  isMixedCase(text) {
    return match(text, /[_]*[a-z$]+[a-zA-Z0-9$]*[_]?/)
  },

  isNotMixedCase(text) {
    return !this.isMixedCase(text)
  },

  isCamelCase(text) {
    return match(text, /[A-Z$]+[a-zA-Z0-9$]*/)
  },

  isNotCamelCase(text) {
    return !this.isCamelCase(text)
  },

  isUpperSnakeCase(text) {
    return match(text, /_{0,2}[A-Z0-9$]+[_A-Z0-9$]*/)
  },

  isNotUpperSnakeCase(text) {
    return !this.isUpperSnakeCase(text)
  },

  hasLeadingUnderscore(text) {
    return text && text[0] === '_'
  },

  isFoundryTestCase(text) {
    // this one checks CamelCase after test keyword
    // const regexTest = /^test(Fork)?(Fuzz)?(Fail)?(_)?[A-Z](Revert(If_|When_){1})?\w{1,}$/

    const regexTest = /^test(Fork)?(Fuzz)?(Fail)?(_)?(Revert(If_|When_){1})?\w{1,}$/
    const matchRegexTest = match(text, regexTest)

    // this one checks CamelCase after test keyword
    // const regexInvariant = /^(invariant|statefulFuzz)(_)?[A-Z]\w{1,}$/
    const regexInvariant = /^(invariant|statefulFuzz)(_)?\w{1,}$/
    const matchRegexInvariant = match(text, regexInvariant)

    return matchRegexTest || matchRegexInvariant
  },
}
