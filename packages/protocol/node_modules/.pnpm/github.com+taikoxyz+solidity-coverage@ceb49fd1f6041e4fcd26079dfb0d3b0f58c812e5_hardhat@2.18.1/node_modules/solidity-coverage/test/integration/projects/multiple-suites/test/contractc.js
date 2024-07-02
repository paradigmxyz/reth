const ContractC = artifacts.require("ContractC");

contract("contractc", function(accounts) {
  let instance;

  before(async () => instance = await ContractC.new())

  it('sends', async function(){
    await instance.sendFn();
  });

  it('calls', async function(){
    await instance.callFn();
  })

  it('sends', async function(){
    await instance.sendFn();
  });

});
