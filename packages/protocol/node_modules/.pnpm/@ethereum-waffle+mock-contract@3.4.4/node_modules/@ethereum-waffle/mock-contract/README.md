![Ethereum Waffle](https://raw.githubusercontent.com/EthWorks/Waffle/master/docs/source/logo.png)

[![CircleCI](https://circleci.com/gh/EthWorks/Waffle.svg?style=svg)](https://circleci.com/gh/EthWorks/Waffle)
[![](https://img.shields.io/npm/v/@ethereum-waffle/mock-contract.svg)](https://www.npmjs.com/package/@ethereum-waffle/mock-contract)

# @ethereum-waffle/mock-contract

Library for mocking smart contract dependencies during unit testing.

## Installation
In the current version of waffle (v3.x.x) you will install this package as a dependency of the main waffle package - `ethereum-waffle`.

```
yarn add --dev ethereum-waffle
npm install --save-dev ethereum-waffle
```

If you want to use this package directly please install it via:
```
yarn add --dev @ethereum-waffle/mock-contract
npm install --save-dev @ethereum-waffle/mock-contract
```

## Usage

Create an instance of a mock contract providing the ABI/interface of the smart contract you want to mock:

```js
const {deployMockContract} = require('@ethereum-waffle/mock-contract');

...

const mockContract = await deployMockContract(wallet, contractAbi);
```

Mock contract can now be passed into other contracts by using the `address` attribute.

Return values for mocked functions can be set using:

```js
await mockContract.mock.<nameOfMethod>.returns(<value>)
await mockContract.mock.<nameOfMethod>.withArgs(<arguments>).returns(<value>)
```

Methods can also be set up to be reverted using:

```js
await mockContract.mock.<nameOfMethod>.reverts()
await mockContract.mock.<nameOfMethod>.withArgs(<arguments>).reverts()
```

Sometimes you may have an overloaded function name:

```solidity
contract OverloadedFunctions is Ownable {
  function burn(uint256 amount) external returns (bool) {
    // ...
  }

  function burn(address user, uint256 amount) external onlyOwner returns (bool) {
    // ...
  }
}
```

You may choose which function to call by using its signature:

```js
await mockContract.mock['burn(uint256)'].returns(true)
await mockContract.mock['burn(address,uint256)'].withArgs('0x1234...', 1000).reverts()
```

You may wish to execute another contract through a mock.  Given the "AmIRichAlready" code below, you could call constant functions using `staticcall`:

```js
const contractFactory = new ContractFactory(AmIRichAlready.abi, AmIRichAlready.bytecode, sender);
const amIRich = await contractFactory.deploy()
const mockERC20 = await deployMockContract(sender, IERC20.abi);

let result = await mockERC20.staticcall(amIRich, 'check()')
// you may also just use the function name
result = await mockERC20.staticcall(amIRich, 'check')
expect(result).to.equal(true) // result will be true if you have enough tokens
```

You may also execute transactions through the mock, using `call`:

```js
const contractFactory = new ContractFactory(AmIRichAlready.abi, AmIRichAlready.bytecode, sender);
const amIRich = await contractFactory.deploy()
const mockERC20 = await deployMockContract(sender, IERC20.abi);

let result = await mockERC20.call(amIRich, 'setRichness(uint256)', 1000)
// you may also just use the function name
result = await mockERC20.call(amIRich, 'setRichness', 1000)
expect(await amIRich.richness()).to.equal('1000') // richness was updated
```

## Example

The example below illustrates how `mock-contract` can be used to test the very simple `AmIRichAlready` contract.

```Solidity
pragma solidity ^0.6.0;

interface IERC20 {
    function balanceOf(address account) external view returns (uint256);
}

contract AmIRichAlready {
    IERC20 private tokenContract;
    uint public richness = 1000000 * 10 ** 18;

    constructor (IERC20 _tokenContract) public {
        tokenContract = _tokenContract;
    }

    function check() public view returns (bool) {
        uint balance = tokenContract.balanceOf(msg.sender);
        return balance > richness;
    }

    function setRichness(uint256 _richness) public {
      richness = _richness;
    }
}
```

We are mostly interested in the `tokenContract.balanceOf` call. Mock contract will be used to mock exactly this call with values that are significant for the return of the `check()` method.

```js
import {use, expect} from 'chai';
import {Contract, ContractFactory, utils, Wallet} from 'ethers';
import {MockProvider} from '@ethereum-waffle/provider';
import {waffleChai} from '@ethereum-waffle/chai';
import {deployMockContract} from '@ethereum-waffle/mock-contract';

import IERC20 from './helpers/interfaces/IERC20.json';
import AmIRichAlready from './helpers/interfaces/AmIRichAlready.json';

use(waffleChai);

describe('Am I Rich Already', () => {
  let contractFactory: ContractFactory;
  let sender: Wallet;
  let receiver: Wallet;
  let mockERC20: Contract;
  let contract: Contract;

  beforeEach(async () => {
    [sender, receiver] = new MockProvider().getWallets();
    mockERC20 = await deployMockContract(sender, IERC20.abi);
    contractFactory = new ContractFactory(AmIRichAlready.abi, AmIRichAlready.bytecode, sender);
    contract = await contractFactory.deploy(mockERC20.address);
  });

  it('returns false if the wallet has less then 1000000 coins', async () => {
    await mockERC20.mock.balanceOf.returns(utils.parseEther('999999'));
    expect(await contract.check()).to.be.equal(false);
  });

  it('returns true if the wallet has at least 1000000 coins', async () => {
    await mockERC20.mock.balanceOf.returns(utils.parseEther('1000001'));
    expect(await contract.check()).to.equal(true);
  });

  it('reverts if the ERC20 reverts', async () => {
    await mockERC20.mock.balanceOf.reverts();
    await expect(contract.check()).to.be.revertedWith('Mock revert');
  });

  it('returns 1000001 coins for my address and 0 otherwise', async () => {
    await mockERC20.mock.balanceOf.returns('0');
    await mockERC20.mock.balanceOf.withArgs(sender.address).returns(utils.parseEther('1000001'));

    expect(await contract.check()).to.equal(true);
    expect(await contract.connect(receiver.address).check()).to.equal(false);
  });
});
```

# Special thanks

Special thanks to @spherefoundry for creating the original [Doppelganger](https://github.com/EthWorks/Doppelganger) project.
