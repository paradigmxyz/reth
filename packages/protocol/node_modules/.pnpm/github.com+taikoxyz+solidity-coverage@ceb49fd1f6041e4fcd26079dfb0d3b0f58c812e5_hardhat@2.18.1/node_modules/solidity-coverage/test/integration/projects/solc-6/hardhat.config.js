require("@nomiclabs/hardhat-truffle5");
require(__dirname + "/../plugins/nomiclabs.plugin");

module.exports = {
  solidity: {
    version: "0.6.7"
  },
  logger: process.env.SILENT ? { log: () => {} } : console,
};
