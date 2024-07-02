const OnlyCall = artifacts.require('OnlyCall');

contract('OnlyCall', accounts => {
  it('should return val + 2', async function(){
    const onlycall = await OnlyCall.new();
    const val = await onlycall.addTwo(5);
    assert.equal(val.toNumber(), 7);
  })
});
