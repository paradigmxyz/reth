const ContractA = artifacts.require("ContractA");
const ContractB = artifacts.require("ContractB");

contract("contracts", function(accounts) {
  let instanceA;
  let instanceB;

  before(async () => {
    instanceA = await ContractA.new();
    instanceB = await ContractB.new();
  });

  it('A sends', async function(){
    await instanceA.sendFn();
  });

  it('A calls', async function(){
    await instanceA.callFn();
  });

  it('B sends', async function(){
    await instanceB.sendFn();
  });

  it('B calls', async function(){
    await instanceB.callFn();
  });

});
