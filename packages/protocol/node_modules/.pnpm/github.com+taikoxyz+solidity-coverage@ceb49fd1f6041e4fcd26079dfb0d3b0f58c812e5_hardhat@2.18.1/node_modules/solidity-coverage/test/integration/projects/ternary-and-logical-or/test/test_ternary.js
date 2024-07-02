const Contract_ternary = artifacts.require("Contract_ternary");

contract("contract_ternary", function(accounts) {
  let instance;

  before(async () => instance = await Contract_ternary.new())

  it('misc ternary conditionals', async function(){
    await instance.a();
    await instance.b();
    await instance.c();
    await instance.d();
    await instance.e();
    await instance.f();
  });
});
