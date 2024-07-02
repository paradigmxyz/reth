module.exports = {
  silent: process.env.SILENT ? true : false,
  skipFiles: ['skipped-folder'],
  istanbulReporter: ['json-summary', 'text'],
  configureYulOptimizer: true,
  solcOptimizerDetails: {
    peephole: false,
    inliner: false,
    jumpdestRemover: false,
    orderLiterals: true,  // <-- TRUE! Stack too deep when false
    deduplicate: false,
    cse: false,
    constantOptimizer: false,
    yul: false
  }
}
