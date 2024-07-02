const UsesPure = artifacts.require("UsesPure");
const CLibrary = artifacts.require("CLibrary");

module.exports = function(deployer) {
  deployer.deploy(CLibrary);
  deployer.link(CLibrary, UsesPure);
  deployer.deploy(UsesPure);
};