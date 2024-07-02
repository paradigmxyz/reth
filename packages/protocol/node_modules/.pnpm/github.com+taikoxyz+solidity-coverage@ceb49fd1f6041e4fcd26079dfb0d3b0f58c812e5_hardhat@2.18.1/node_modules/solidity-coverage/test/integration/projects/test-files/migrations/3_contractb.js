const ContractB = artifacts.require("ContractB");

module.exports = function(deployer) {
  deployer.deploy(ContractB);
};