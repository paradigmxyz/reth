const Expensive = artifacts.require('Expensive');

contract('Expensive', () => {
  it('should deploy', async function() {
    const instance = await Expensive.new()
    const hash = instance.transactionHash;
    const receipt = await web3.eth.getTransactionReceipt(hash);
    assert(receipt.gasUsed > 20000000)
  });
});
