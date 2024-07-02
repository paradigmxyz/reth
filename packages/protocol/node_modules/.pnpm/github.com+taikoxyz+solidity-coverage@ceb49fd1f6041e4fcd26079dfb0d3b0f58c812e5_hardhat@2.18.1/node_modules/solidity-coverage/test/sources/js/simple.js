const Simple = artifacts.require('Simple');

contract('Simple', () => {
  it('should set x to 5', async function(){
    const simple = await Simple.new()
    await simple.test(5);
    const val = await simple.getX.call();
    assert.equal(val.toNumber(), 5);
  });
});
