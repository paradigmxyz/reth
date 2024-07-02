module.exports = {
  networks: {
    local: {
      host: "localhost",
      port: 9545,
      network_id: "*",
      gas: 4000000,
    },
    ropsten: {
      host: "localhost",
      port: 8545,
      network_id: "3",
      from: "0xa303ddc620aa7d1390baccc8a495508b183fab59",
      gas: 5000000,
    }
  },
  mocha: {
    reporter: 'eth-gas-reporter',
    reporterOptions : {
      currency: 'USD',
      gasPrice: 1
    }
  },
/*  solc: {
    optimizer: {
      enabled: true,
      runs: 200
    }
  }*/
};
