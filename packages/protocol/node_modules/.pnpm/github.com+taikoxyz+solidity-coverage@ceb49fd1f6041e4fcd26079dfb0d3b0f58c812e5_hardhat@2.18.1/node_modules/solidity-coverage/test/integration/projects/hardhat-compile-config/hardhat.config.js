require("@nomiclabs/hardhat-truffle5");
require(__dirname + "/../plugins/nomiclabs.plugin");

module.exports={
  solidity: {
    compilers: [
      {
        version: "0.5.5"
      },
      {
        version: "0.5.7"
      },
      // Make sure optimizer gets disabled
      {
        version: "0.6.7",
        settings: {
          optimizer: {
            enabled: true,
            runs: 200
          }
        }
      }
    ],
    overrides: {
      "contracts/ContractA.sol": {
        version: "0.5.5",
        settings: {
          optimizer: {
            enabled: true,
            runs: 200
          }
        }
      }
    }
  },
  logger: process.env.SILENT ? { log: () => {} } : console,
};
