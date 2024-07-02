const Contract_OR = artifacts.require("Contract_OR");

contract("contract_or", function(accounts) {
  let instance;

  before(async () => instance = await Contract_OR.new())

  it('_if', async function(){
    await instance._if(0);
    await instance._if(7);
  });

  it('_if_and', async function(){
    await instance._if_and(1);
  });

  it('_return', async function(){
    await instance._return(4);
  });

  it('_while', async function(){
    await instance._while(1);
  });

  it('_require', async function(){
    await instance._require(2);
  })

  it('_require_multi_line', async function(){
    await instance._require_multi_line(1);
    await instance._require_multi_line(3);
  })

  it('_if_neither', async function(){
    await instance._if_neither(3);
  })
});
