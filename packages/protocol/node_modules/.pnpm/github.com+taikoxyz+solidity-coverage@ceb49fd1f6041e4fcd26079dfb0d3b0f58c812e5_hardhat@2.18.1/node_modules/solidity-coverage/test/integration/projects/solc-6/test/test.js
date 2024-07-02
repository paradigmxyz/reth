const ContractA = artifacts.require("ContractA");

contract("contracta", function(accounts) {
  let instance;

  before(async () => instance = await ContractA.new())

  it('simpleSet (overridden method)', async function(){
    await instance.simpleSet(5);
  });

  it('simpleView (overridden modifier)', async function(){
    await instance.simpleView(5);
  });

  it('tryCatch', async function(){
    await instance.tryCatch();
  });

  it('arraySlice', async function(){
    await instance.arraySlice(5,7);
  });

  it('payableFn', async function(){
    await instance.payableFn();
  })
});
