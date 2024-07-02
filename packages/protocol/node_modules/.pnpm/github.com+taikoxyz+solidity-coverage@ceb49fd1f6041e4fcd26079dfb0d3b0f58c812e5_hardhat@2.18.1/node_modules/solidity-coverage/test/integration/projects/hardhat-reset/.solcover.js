// Testing hooks
const fn = (msg, config) => config.logger.log(msg);

module.exports = {
  skipFiles: ['Migrations.sol'],
  silent: process.env.SILENT ? true : false,
  istanbulReporter: ['json-summary', 'text'],
}