const Wallet = artifacts.require('Wallet');

contract('Wallet', accounts => {
  it('should should allow transfers and sends', async () => {
    const walletA = await Wallet.new();
    const walletB = await Wallet.new();

    await walletA.sendTransaction({
      value: web3.utils.toBN(500), from: accounts[0],
    });

    await walletA.sendPayment(50, walletB.address, {
      from: accounts[0],
    });

    await walletA.transferPayment(50, walletB.address, {
      from: accounts[0],
    });
  });
});