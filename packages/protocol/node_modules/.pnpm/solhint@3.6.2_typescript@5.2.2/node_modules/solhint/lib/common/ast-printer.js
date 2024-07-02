const antlr4 = require('antlr4')

class AstPrinter {
  constructor(tokenStream) {
    this.tokenStream = tokenStream
  }

  print(ctx) {
    this.explore(ctx, 0)
  }

  explore(ctx, indentation) {
    const ruleName = ctx.parser.ruleNames[ctx.ruleIndex]

    console.log('  '.repeat(indentation + 1) + ruleName + ' ' + ctx.getText())

    for (let i = 0; i < ctx.getChildCount(); i++) {
      const element = ctx.getChild(i)

      if (element instanceof antlr4.ParserRuleContext) {
        this.explore(element, indentation + 1)
      } else {
        console.log(
          '  '.repeat(indentation + 2) + element.constructor.name + ' ' + element.getText()
        )
      }
    }
  }
}

module.exports = AstPrinter
