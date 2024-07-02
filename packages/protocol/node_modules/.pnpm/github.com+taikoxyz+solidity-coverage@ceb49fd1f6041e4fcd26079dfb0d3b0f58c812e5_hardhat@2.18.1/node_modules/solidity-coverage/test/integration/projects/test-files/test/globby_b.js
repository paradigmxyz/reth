const ContractB = artifacts.require("ContractB");

contract("contractB", function(accounts) {
  let instance;

  before(async () => instance = await ContractB.new())

  it('sends', async function(){
    await instance.sendFn();
  });

  it('calls', async function(){
    await instance.callFn();
  })
});
