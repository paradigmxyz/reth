module.exports = {
  silent: process.env.SILENT ? true : false,
  skipFiles: ['skipped-folder'],
  istanbulReporter: ['json-summary', 'text'],
  configureYulOptimizer: true
}
