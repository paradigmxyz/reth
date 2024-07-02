const CodeComplexityChecker = require('./code-complexity')
const FunctionMaxLinesChecker = require('./function-max-lines')
const MaxLineLengthChecker = require('./max-line-length')
const MaxStatesCountChecker = require('./max-states-count')
const NoEmptyBlocksChecker = require('./no-empty-blocks')
const NoUnusedVarsChecker = require('./no-unused-vars')
const PayableFallbackChecker = require('./payable-fallback')
const ReasonStringChecker = require('./reason-string')
const NoConsoleLogChecker = require('./no-console')
const NoGlobalImportsChecker = require('./no-global-import')
const NoUnusedImportsChecker = require('./no-unused-import')
const ExplicitTypesChecker = require('./explicit-types')
const CustomErrorsChecker = require('./custom-errors')
const OneContractPerFileChecker = require('./one-contract-per-file')

module.exports = function checkers(reporter, config, inputSrc, tokens) {
  return [
    new CodeComplexityChecker(reporter, config),
    new FunctionMaxLinesChecker(reporter, config),
    new MaxLineLengthChecker(reporter, config, inputSrc),
    new MaxStatesCountChecker(reporter, config),
    new NoEmptyBlocksChecker(reporter),
    new NoUnusedVarsChecker(reporter),
    new PayableFallbackChecker(reporter),
    new ReasonStringChecker(reporter, config),
    new NoConsoleLogChecker(reporter),
    new NoGlobalImportsChecker(reporter),
    new NoUnusedImportsChecker(reporter, tokens),
    new ExplicitTypesChecker(reporter, config),
    new CustomErrorsChecker(reporter),
    new OneContractPerFileChecker(reporter),
  ]
}
