const _ = require('lodash')

function tokens(ctx) {
  let curCtx = ctx
  while (curCtx && curCtx.parentCtx && !curCtx.parser) {
    curCtx = curCtx.parentCtx
  }

  return curCtx.parser._input.tokens
}

function prevToken(ctx) {
  return prevTokenFromToken(tokens(ctx), startOf(ctx))
}

function prevTokenFromToken(tokenList, curToken) {
  const tokenIndex = _.sortedIndexBy(tokenList, curToken, _.property('start'))

  let prevTokenIndex = tokenIndex - 1
  while (prevTokenIndex >= 0 && tokenList[prevTokenIndex].channel !== 0) {
    prevTokenIndex -= 1
  }

  return tokenList[prevTokenIndex]
}

function nextToken(ctx) {
  return nextTokenFromToken(tokens(ctx), stopOf(ctx))
}

function nextTokenFromToken(tokenList, curToken) {
  const tokenIndex = _.sortedIndexBy(tokenList, curToken, _.property('start'))

  let nextTokenIndex = tokenIndex + 1
  while (nextTokenIndex < tokenList.length && tokenList[nextTokenIndex].channel !== 0) {
    nextTokenIndex += 1
  }

  return tokenList[nextTokenIndex]
}

function hasNoSpaceAfter(ctx) {
  return noSpaces(nextToken(ctx), stopOf(ctx))
}

function hasNoSpacesBefore(ctx) {
  return noSpaces(startOf(ctx), prevToken(ctx))
}

function hasSpaceBefore(ctx) {
  return hasSpace(startOf(ctx), prevToken(ctx))
}

function hasSpaceAfter(ctx) {
  return hasSpace(nextToken(ctx), stopOf(ctx))
}

function noSpaces(token1, token2) {
  return hasDiff(token1, token2, 1)
}

function hasSpace(token1, token2) {
  return hasDiff(token1, token2, 2)
}

function hasDiff(token1, token2, indent) {
  return token1.start - token2.stop === indent || !onSameLine(token1, token2)
}

function onSameLine(token1, token2) {
  return token1.line === token2.line
}

function startOf(ctx) {
  return ctx.start || ctx.symbol
}

function stopOf(ctx) {
  return ctx.stop || ctx.symbol
}

function columnOf(ctx) {
  return startOf(ctx).column
}

function lineOf(ctx) {
  return startOf(ctx).line
}

function stopLine(ctx) {
  return stopOf(ctx).line
}

class BaseTokenList {
  static from(ctx) {
    const constructor = this
    return new constructor(ctx)
  }

  constructor(ctx) {
    this.tokens = tokens(ctx)
  }
}

class Token {
  constructor(tokens, curToken) {
    this.curToken = curToken
    this.tokens = tokens
  }

  get line() {
    return this.curToken.line
  }

  get column() {
    return this.curToken.column
  }
}

const AlignValidatable = (type) =>
  class extends type {
    isIncorrectAligned() {
      return !this.isCorrectAligned()
    }
  }

module.exports = {
  hasNoSpaceAfter,
  hasNoSpacesBefore,
  hasSpaceAfter,
  hasSpaceBefore,
  onSameLine,
  prevToken,
  startOf,
  stopOf,
  columnOf,
  lineOf,
  stopLine,
  hasSpace,
  noSpaces,
  tokens,
  nextTokenFromToken,
  prevTokenFromToken,
  BaseTokenList,
  Token,
  AlignValidatable,
}
