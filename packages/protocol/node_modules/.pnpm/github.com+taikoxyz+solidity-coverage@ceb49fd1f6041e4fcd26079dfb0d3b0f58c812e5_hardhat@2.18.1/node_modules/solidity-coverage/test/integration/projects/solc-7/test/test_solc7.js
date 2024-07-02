const ContractA = artifacts.require("ContractA");
const ContractB = artifacts.require("ContractB");

contract("contracta", function(accounts) {
  let a,b;

  before(async () => {
    a = await ContractA.new();
    b = await ContractB.new();
  })

  it('a:addFive', async function(){
    await a.addFive();
  });

  it('a:addSeven', async function(){
    //await a.addSeven();
  });

  it('b:addTen', async function(){
    await b.addTen();
  })
});
