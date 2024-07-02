const AvoidCallValueChecker = require('./avoid-call-value')
const AvoidLowLevelCallsChecker = require('./avoid-low-level-calls')
const AvoidSha3Checker = require('./avoid-sha3')
const AvoidSuicideChecker = require('./avoid-suicide')
const AvoidThrowChecker = require('./avoid-throw')
const AvoidTxOriginChecker = require('./avoid-tx-origin')
const CheckSendResultChecker = require('./check-send-result')
const CompilerVersionChecker = require('./compiler-version')
const FuncVisibilityChecker = require('./func-visibility')
const MarkCallableContractsChecker = require('./mark-callable-contracts')
const MultipleSendsChecker = require('./multiple-sends')
const NoComplexFallbackChecker = require('./no-complex-fallback')
const NoInlineAssemblyChecker = require('./no-inline-assembly')
const NotRelyOnBlockHashChecker = require('./not-rely-on-block-hash')
const NotRelyOnTimeChecker = require('./not-rely-on-time')
const ReentrancyChecker = require('./reentrancy')
const StateVisibilityChecker = require('./state-visibility')

module.exports = function security(reporter, config, inputSrc) {
  return [
    new AvoidCallValueChecker(reporter),
    new AvoidLowLevelCallsChecker(reporter),
    new AvoidSha3Checker(reporter),
    new AvoidSuicideChecker(reporter),
    new AvoidThrowChecker(reporter),
    new AvoidTxOriginChecker(reporter),
    new CheckSendResultChecker(reporter),
    new CompilerVersionChecker(reporter, config),
    new FuncVisibilityChecker(reporter, config),
    new MarkCallableContractsChecker(reporter),
    new MultipleSendsChecker(reporter),
    new NoComplexFallbackChecker(reporter),
    new NoInlineAssemblyChecker(reporter),
    new NotRelyOnBlockHashChecker(reporter),
    new NotRelyOnTimeChecker(reporter),
    new ReentrancyChecker(reporter, inputSrc),
    new StateVisibilityChecker(reporter),
  ]
}
