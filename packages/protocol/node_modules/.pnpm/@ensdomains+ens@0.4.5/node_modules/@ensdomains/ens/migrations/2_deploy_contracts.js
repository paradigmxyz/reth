const ENS = artifacts.require("./ENSRegistry.sol");
const FIFSRegistrar = artifacts.require('./FIFSRegistrar.sol');

// Currently the parameter('./ContractName') is only used to imply
// the compiled contract JSON file name. So even though `Registrar.sol` is
// not existed, it's valid to put it here.
// TODO: align the contract name with the source code file name.
const Registrar = artifacts.require('./HashRegistrar.sol');
const web3 = new (require('web3'))();
const namehash = require('eth-ens-namehash');

/**
 * Calculate root node hashes given the top level domain(tld)
 *
 * @param {string} tld plain text tld, for example: 'eth'
 */
function getRootNodeFromTLD(tld) {
  return {
    namehash: namehash(tld),
    sha3: web3.sha3(tld)
  };
}

/**
 * Deploy the ENS and FIFSRegistrar
 *
 * @param {Object} deployer truffle deployer helper
 * @param {string} tld tld which the FIFS registrar takes charge of
 */
function deployFIFSRegistrar(deployer, tld) {
  var rootNode = getRootNodeFromTLD(tld);

  // Deploy the ENS first
  deployer.deploy(ENS)
    .then(() => {
      // Deploy the FIFSRegistrar and bind it with ENS
      return deployer.deploy(FIFSRegistrar, ENS.address, rootNode.namehash);
    })
    .then(function() {
      // Transfer the owner of the `rootNode` to the FIFSRegistrar
      return ENS.at(ENS.address).then((c) => c.setSubnodeOwner('0x0', rootNode.sha3, FIFSRegistrar.address));
    });
}

/**
 * Deploy the ENS and HashRegistrar(Simplified)
 *
 * @param {Object} deployer truffle deployer helper
 * @param {string} tld tld which the Hash registrar takes charge of
 */
function deployAuctionRegistrar(deployer, tld) {
  var rootNode = getRootNodeFromTLD(tld);

  // Deploy the ENS first
  deployer.deploy(ENS)
    .then(() => {
      // Deploy the HashRegistrar and bind it with ENS
      // The last argument `0` specifies the auction start date to `now`
      return deployer.deploy(Registrar, ENS.address, rootNode.namehash, 0);
    })
    .then(function() {
      // Transfer the owner of the `rootNode` to the HashRegistrar
      return ENS.at(ENS.address).then((c) => c.setSubnodeOwner('0x0', rootNode.sha3, Registrar.address));
    });
}

module.exports = function(deployer, network) {
  var tld = 'eth';

  if (network === 'dev.fifs') {
    deployFIFSRegistrar(deployer, tld);
  }
  else if (network === 'dev.auction') {
    deployAuctionRegistrar(deployer, tld);
  }

};
