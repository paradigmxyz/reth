const ContractA = artifacts.require("ContractA");

contract("contracta", function(accounts) {
  let a,b;

  before(async () => {
    a = await ContractA.new();
  })

  it('a:throwError', async function(){
    await a.throwError(a.address);
  });

  it('a:checkSomething', async function(){
    await a.checkSomething();
  })
});
