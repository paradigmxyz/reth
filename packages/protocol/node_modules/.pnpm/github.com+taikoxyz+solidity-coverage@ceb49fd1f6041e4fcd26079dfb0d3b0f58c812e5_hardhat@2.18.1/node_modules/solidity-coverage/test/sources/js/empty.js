const Empty = artifacts.require('Empty');

contract('Empty', function() {
  it('should deploy', async function (){
    await Empty.new()
  });
});
