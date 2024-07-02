const Modified = artifacts.require('Modified');

contract('Modified', () => {
  it('should set counter', async function(){
    const m = await Modified.new()
    await m.set(5);
  });
});
