const createInfuraProvider = require('eth-json-rpc-infura/src/createProvider')
const ProviderSubprovider = require('./provider.js')

class InfuraSubprovider extends ProviderSubprovider {
  constructor(opts = {}) {
    const provider = createInfuraProvider(opts)
    super(provider)
  }
}

module.exports = InfuraSubprovider
