const ContractA = artifacts.require("ContractA");

module.exports = function(deployer) {
  deployer.deploy(ContractA);
};