require("@nomiclabs/hardhat-truffle5");
require(__dirname + "/../plugins/nomiclabs.plugin");

module.exports={
  logger: process.env.SILENT ? { log: () => {} } : console,
};
