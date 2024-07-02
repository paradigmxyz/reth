const MatrixA = artifacts.require("MatrixA");

contract("MatrixA", function(accounts) {
  let instance;

  before(async () => instance = await MatrixA.new())

  it('sends', async function(){
    await instance.sendFn();
  });

  it('calls', async function(){
    await instance.callFn();
  })
});
