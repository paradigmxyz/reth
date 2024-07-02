const Simple = artifacts.require('Simple');

contract('Simple', accounts => {

  it('should load with ~ expected balance', async function(){
    let balance = await web3.eth.getBalance(accounts[0]);
    balance = web3.utils.fromWei(balance);
    assert(parseInt(balance) >= 776)
  });
});
