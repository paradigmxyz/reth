const QuotesChecker = require('./quotes')
const ComprehensiveInterfaceChecker = require('./comprehensive-interface')

module.exports = function checkers(reporter, config, tokens) {
  return [
    new QuotesChecker(reporter, config, tokens),
    new ComprehensiveInterfaceChecker(reporter, config, tokens),
  ]
}
