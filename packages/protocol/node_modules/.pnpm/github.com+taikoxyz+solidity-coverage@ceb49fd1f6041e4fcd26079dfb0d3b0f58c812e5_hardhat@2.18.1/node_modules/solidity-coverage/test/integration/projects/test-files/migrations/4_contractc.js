const ContractC = artifacts.require("ContractC");

module.exports = function(deployer) {
  deployer.deploy(ContractC);
};