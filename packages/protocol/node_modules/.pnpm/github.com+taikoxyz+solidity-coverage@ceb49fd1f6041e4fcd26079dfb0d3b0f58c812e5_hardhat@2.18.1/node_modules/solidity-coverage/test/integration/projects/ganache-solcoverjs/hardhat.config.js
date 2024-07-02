require("@nomiclabs/hardhat-truffle5");
require(__dirname + "/../plugins/nomiclabs.plugin");

module.exports = {
  solidity: {
    version: "0.7.3"
  },
  networks: {
    coverage: {
      url: "http://127.0.0.1:8555"
    }
  }.
  logger: process.env.SILENT ? { log: () => {} } : console,
};
