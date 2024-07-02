const _ = require('lodash')
const { hasNoSpaceAfter, hasNoSpacesBefore, hasSpaceAfter, hasSpaceBefore } = require('./tokens')

class StatementsIndentValidator {
  constructor(ctx) {
    this.ctx = ctx
  }

  cases(...cases) {
    this.casesList = cases

    return this
  }

  errorsTo(callback) {
    const validCase = this._validCase()
    if (validCase) {
      validCase.validate(_.once(callback))
    }
  }

  _validCase() {
    for (const curCase of this.casesList) {
      if (curCase.syntaxMatch(this.ctx)) {
        return curCase
      }
    }

    return null
  }
}

class SyntacticSequence {
  constructor() {
    this.items = []
  }

  spaceAroundOrNot(...terms) {
    this.items.push(new SpaceAroundOrNot(...terms))

    return this
  }

  spaceAround(...terms) {
    this.items.push(new SpaceAround(...terms))

    return this
  }

  rule(name) {
    this.items.push(new Rule(name))

    return this
  }

  term(...termNames) {
    this.items.push(new Term(...termNames))

    return this
  }

  noSpaces() {
    const items = this.items
    items.push(new NoSpaces(this.lastItem))

    return this
  }

  space() {
    const items = this.items
    items.push(new Space(this.lastItem))

    return this
  }

  noSpacesAround(...names) {
    this.items.push(new NoSpacesAround(...names))

    return this
  }

  expression() {
    return this.rule('expression')
  }

  identifier() {
    return this.rule('identifier')
  }

  statement() {
    return this.rule('statement')
  }

  errorsTo(ctx, callback) {
    if (this.syntaxMatch(ctx)) {
      this.validate(_.once(callback))
    }
  }

  syntaxMatch(ctx) {
    const childs = ctx.children
    const syntaxMatchers = this.items.filter((i) => i.syntaxMatch)

    if (!childs || childs.length === 0 || syntaxMatchers.length !== childs.length) {
      return false
    }

    for (let i = 0; i < childs.length; i += 1) {
      if (!syntaxMatchers[i].syntaxMatch(childs[i])) {
        return false
      }
    }

    return true
  }

  validate(callback) {
    return this.items.filter((i) => i.listenError).forEach((i) => i.listenError(callback))
  }

  get lastItem() {
    const items = this.items

    return items[items.length - 1]
  }
}

class Term {
  static term(...terms) {
    return new SyntacticSequence().term(...terms)
  }

  constructor(...terms) {
    this.terms = terms
  }

  syntaxMatch(ctx) {
    if (this.terms.includes(ctx.getText())) {
      this.ctx = ctx
      return true
    } else {
      return false
    }
  }
}

class Rule {
  static rule(name) {
    return new SyntacticSequence().rule(name)
  }

  static expression() {
    return Rule.rule('expression')
  }

  constructor(ruleName) {
    this.ruleName = ruleName
  }

  _ruleToClassName() {
    return this.ruleName[0].toUpperCase() + this.ruleName.substr(1) + 'Context'
  }

  syntaxMatch(ctx) {
    if (ctx.constructor.name === this._ruleToClassName()) {
      this.ctx = ctx

      return true
    } else {
      return false
    }
  }
}

class Space {
  constructor(prevRule) {
    this.prevRule = prevRule
  }

  listenError(callback) {
    const ctx = this.prevRule.ctx
    spaceAfter(ctx, callback)
  }
}

class NoSpaces {
  constructor(prevRule) {
    this.prevRule = prevRule
  }

  listenError(callback) {
    noSpacesAfter(this.prevRule.ctx, callback)
  }
}

class NoSpacesAround extends Term {
  listenError(callback) {
    const ctx = this.ctx

    noSpacesBefore(ctx, callback)
    noSpacesAfter(ctx, callback)
  }
}

class SpaceAroundOrNot extends Term {
  listenError(callback) {
    const ctx = this.ctx
    const hasSpacesAround = hasSpaceBefore(ctx) && hasSpaceAfter(ctx)
    const hasNoSpacesAround = hasNoSpacesBefore(ctx) && hasNoSpaceAfter(ctx)

    if (!hasNoSpacesAround && !hasSpacesAround) {
      if (callback) {
        callback(ctx, 'Use the same amount of whitespace on either side of an operator.')
      }
    }
  }
}

class SpaceAround extends Term {
  listenError(callback) {
    const ctx = this.ctx

    spaceAfter(ctx, callback)
    spaceBefore(ctx, callback)
  }
}

function noSpacesAfter(ctx, thenRaise) {
  whenNot(hasNoSpaceAfter, ctx, thenRaise, 'Required no spaces after')
}

function noSpacesBefore(ctx, thenRaise) {
  whenNot(hasNoSpacesBefore, ctx, thenRaise, 'Required no spaces before')
}

function spaceBefore(ctx, thenRaise) {
  whenNot(hasSpaceBefore, ctx, thenRaise, 'Required space before')
}

function spaceAfter(ctx, thenRaise) {
  whenNot(hasSpaceAfter, ctx, thenRaise, 'Required space after')
}

function whenNot(conditionFn, ctx, callback, message) {
  if (!conditionFn(ctx)) {
    if (callback) {
      callback(ctx, `${message} ${ctx.getText()}.`)
    }
  }
}

module.exports = { StatementsIndentValidator, Rule, Term }
