// Testing hooks
const fn = (msg, config) => config.logger.log(msg);
const reporterPath = (process.env.TRUFFLE_TEST)
  ? "./plugins/resources/matrix.js"
  : "../plugins/resources/matrix.js";

module.exports = {
  // This is loaded directly from `./plugins` during unit tests. The default val is
  // "solidity-coverage/plugins/resources/matrix.js"
  matrixReporterPath: reporterPath,
  matrixOutputPath: "alternateTestMatrix.json",
  mochaJsonOutputPath: "alternateMochaOutput.json",

  skipFiles: ['Migrations.sol'],
  silent: process.env.SILENT ? true : false,
  istanbulReporter: ['json-summary', 'text'],
}
