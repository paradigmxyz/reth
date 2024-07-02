const usingOraclize = artifacts.require('usingOraclize');

contract('Oraclize', function(accounts){
  it('oraclize', async function(){
    const ora = await usingOraclize.new();
    await ora.test();
  });
});

