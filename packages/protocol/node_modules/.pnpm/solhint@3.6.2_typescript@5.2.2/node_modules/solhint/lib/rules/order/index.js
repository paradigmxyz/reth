const FuncOrderChecker = require('./func-order')
const ImportsOnTopChecker = require('./imports-on-top')
const VisibilityModifierOrderChecker = require('./visibility-modifier-order')
const OrderingChecker = require('./ordering')

module.exports = function order(reporter, tokens) {
  return [
    new FuncOrderChecker(reporter),
    new ImportsOnTopChecker(reporter),
    new VisibilityModifierOrderChecker(reporter, tokens),
    new OrderingChecker(reporter),
  ]
}
