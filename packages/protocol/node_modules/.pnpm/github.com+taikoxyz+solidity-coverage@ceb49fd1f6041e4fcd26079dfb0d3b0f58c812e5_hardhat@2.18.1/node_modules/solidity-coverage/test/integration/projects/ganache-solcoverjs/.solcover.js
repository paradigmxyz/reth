module.exports = {
  client: require('ganache-cli'),
  silent: process.env.SILENT ? true : false,
  istanbulReporter: ['json-summary', 'text'],
}
