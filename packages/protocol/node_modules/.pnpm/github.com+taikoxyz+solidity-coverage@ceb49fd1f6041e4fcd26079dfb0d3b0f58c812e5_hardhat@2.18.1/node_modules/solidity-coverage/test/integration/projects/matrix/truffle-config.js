module.exports = {
  networks: {},
  mocha: {},
  compilers: {
    solc: {
      version: "0.7.3"
    }
  },
  logger: process.env.SILENT ? { log: () => {} } : console,
}
