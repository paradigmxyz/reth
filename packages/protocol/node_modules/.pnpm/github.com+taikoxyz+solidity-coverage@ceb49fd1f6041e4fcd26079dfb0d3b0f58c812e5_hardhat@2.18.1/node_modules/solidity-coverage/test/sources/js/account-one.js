const Simple = artifacts.require('Simple');

contract('Simple', () => {
  it('should use 0xe7a46b209a65baadc11bf973c0f4d5f19465ae83', async function(){
    const simple = await Simple.new()
    const result = await simple.test(5);
    assert.equal(result.receipt.from, "0xe7a46b209a65baadc11bf973c0f4d5f19465ae83")
  });
});
