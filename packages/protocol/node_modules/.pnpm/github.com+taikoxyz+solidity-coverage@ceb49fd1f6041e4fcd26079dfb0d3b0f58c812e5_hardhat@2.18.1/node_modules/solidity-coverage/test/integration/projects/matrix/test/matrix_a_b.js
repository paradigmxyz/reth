const MatrixA = artifacts.require("MatrixA");
const MatrixB = artifacts.require("MatrixB");

contract("Matrix A and B", function(accounts) {
  let instanceA;
  let instanceB;

  before(async () => {
    instanceA = await MatrixA.new();
    instanceB = await MatrixB.new();
  })

  it('sends to A', async function(){
    await instanceA.sendFn();
  });

  // Duplicate test title and file should *not* be duplicated in the output
  it('sends to A', async function(){
    await instanceA.sendFn();
  })

  it('calls B', async function(){
    await instanceB.callFn();
  })

  it('sends to B', async function(){
    await instanceB.sendFn();
  });

});
