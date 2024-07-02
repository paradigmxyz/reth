const ContractA = artifacts.require("ContractA");

contract("contracta", function(accounts) {
  let instance;

  before(async () => instance = await ContractA.new())

  it('sends [ @skipForCoverage ]', async function(){
    await instance.sendFn();
  });

  it('calls [ @skipForCoverage ]', async function(){
    await instance.callFn();
  })
});
