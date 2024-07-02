const Owned = artifacts.require('Owned');
const Proxy = artifacts.require('Proxy');

contract('Proxy', accounts => {
  it('when one contract inherits from another', async function(){
    const owned = await Owned.new();
    const proxy = await Proxy.new();
    const val = await proxy.isOwner({from: accounts[0]});
    assert.equal(val, true);
  })
});
