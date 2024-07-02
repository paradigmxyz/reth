const Simple = artifacts.require('Simple');

contract('Simple', () => {
  it('should use 0x42ecc9ab31d7c0240532992682ee3533421dd7f5', async function(){
    const simple = await Simple.new()
    const result = await simple.test(5);
    assert.equal(result.receipt.from, "0x42ecc9ab31d7c0240532992682ee3533421dd7f5")
  });
});
