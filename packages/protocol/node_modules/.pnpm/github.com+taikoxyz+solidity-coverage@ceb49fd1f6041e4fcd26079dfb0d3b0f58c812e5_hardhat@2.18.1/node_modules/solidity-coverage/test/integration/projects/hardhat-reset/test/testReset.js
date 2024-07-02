const { expect } = require("chai");
const { ethers } = require("hardhat");

describe("contractA", function() {
  let instance;
  let startBlockNumber;

  beforeEach(async () => {
    startBlockNumber = await ethers.provider.getBlockNumber();

    await hre.network.provider.request({
        method: "hardhat_reset",
        params: [
          {
            forking: {
              jsonRpcUrl: `https://eth-mainnet.alchemyapi.io/v2/${process.env.ALCHEMY_TOKEN}`,
              blockNumber: startBlockNumber,
            },
          },
        ],
      });

    const factory = await ethers.getContractFactory("ContractA");
    instance = await factory.deploy();
  });

  it('sends', async function(){
    await instance.sendFn();
  });

  it('sends 2', async function(){
    await instance.sendFn2();
  });

  it('calls', async function(){
    await instance.callFn();
  });

  it('calls 2', async function(){
    await instance.callFn2();
  });
});

