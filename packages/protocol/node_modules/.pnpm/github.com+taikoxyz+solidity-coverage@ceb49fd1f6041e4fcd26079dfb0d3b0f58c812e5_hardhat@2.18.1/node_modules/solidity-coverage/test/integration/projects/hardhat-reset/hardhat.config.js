require("@nomiclabs/hardhat-waffle");
require("@nomiclabs/hardhat-ethers");
require(__dirname + "/../plugins/nomiclabs.plugin");

if (!process.env.ALCHEMY_TOKEN){
  throw new Error(
    "This test requires that you set ALCHEMY_TOKEN to a valid token in your development env"
  );
}

module.exports = {
  solidity: {
    version: "0.7.0"
  },
  networks: {
    hardhat: {
      timeout: 100000,
      forking: {
        url: `https://eth-mainnet.alchemyapi.io/v2/${process.env.ALCHEMY_TOKEN}`,
        blockNumber: 14000000,
      }
    }
  },
  mocha: {
    timeout: 100000
  },
  logger: process.env.SILENT ? { log: () => {} } : console,
};
