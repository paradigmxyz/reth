![CI](https://github.com/EthWorks/Waffle/workflows/CI/badge.svg)
[![](https://img.shields.io/npm/v/ethereum-waffle.svg)](https://www.npmjs.com/package/ethereum-waffle)

![Ethereum Waffle](https://raw.githubusercontent.com/EthWorks/Waffle/master/docs/source/logo.png)

The most advanced framework for testing smart contracts.

Sweeter, simpler and faster.

## Links
* Website - https://getwaffle.io/
* Documentation - https://ethereum-waffle.readthedocs.io/

## Philosophy
* __Simpler__: Minimalistic, few dependencies.
* __Sweeter__: Nice syntax, easy to extend.
* __Faster__: Strong focus on the speed of test execution.

## Features:
* Sweet set of chai matchers, e.g.:
  * `expect(...).to.be.revertedWith('Error message')`
  * `expect(...).to.emit(contract, 'EventName').withArgs(...)`)
* Importing contracts from npm modules working out of the box, e.g.:
  * `import "openzeppelin-solidity/contracts/token/ERC20/ERC20.sol";`
* Fixtures that help write fast and maintainable test suites, e.g.:
  * `const {token} = await loadFixture(standardTokenWithBalance);`
* Customizable compilation options with native solc, dockerized solc and any version of solc-js loaded remotely at compiled time
* Mocking smart contracts, e.g.:
  * `const mockToken = await deployMockContract(wallet, IERC20.abi);`
* Support for promise-based configuration, e.g.:
  * use native solc binary for fast compilation in CI environment
  * use solc-js based on contract versions detected (async)
* Support for TypeScript
* Type-safe contract deployment and interactions with TypeChain
* [Documentation](https://ethereum-waffle.readthedocs.io/en/latest/)

## Documentation
Documentation is available [here](https://ethereum-waffle.readthedocs.io/en/latest/).

## Installation:

To get started install `ethereum-waffle` with yarn:
```
yarn add --dev ethereum-waffle
```

Or if you prefer using npm:
```
npm install --save-dev ethereum-waffle
```

## Step by step guide

### Add external dependency:
To add an external library install it using npm:

```sh
npm install @openzeppelin/contracts -D
```

or with yarn:

```sh
yarn add @openzeppelin/contracts -D
```

### Note

Find this example in `examples/basic` and use it.

### Example contract
Below is an example contract written in Solidity. Place it in `contracts/BasicToken.sol` file of your project:

```solidity
pragma solidity ^0.6.0;

import "@openzeppelin/contracts/token/ERC20/ERC20.sol";

// Example class - a mock class using delivering from ERC20
contract BasicToken is ERC20 {
    constructor(uint256 initialBalance) ERC20("Basic", "BSC") public {
        _mint(msg.sender, initialBalance);
    }
}

```

### Example test
Below is an example test written for the contract above compiled with Waffle. Place it under `test/BasicToken.test.ts` file in your project directory:

```ts
import {expect, use} from 'chai';
import {Contract} from 'ethers';
import {deployContract, MockProvider, solidity} from 'ethereum-waffle';
import BasicToken from '../build/BasicToken.json';

use(solidity);

describe('BasicToken', () => {
  const [wallet, walletTo] = new MockProvider().getWallets();
  let token: Contract;

  beforeEach(async () => {
    token = await deployContract(wallet, BasicToken, [1000]);
  });

  it('Assigns initial balance', async () => {
    expect(await token.balanceOf(wallet.address)).to.equal(1000);
  });

  it('Transfer adds amount to destination account', async () => {
    await token.transfer(walletTo.address, 7);
    expect(await token.balanceOf(walletTo.address)).to.equal(7);
  });

  it('Transfer emits event', async () => {
    await expect(token.transfer(walletTo.address, 7))
      .to.emit(token, 'Transfer')
      .withArgs(wallet.address, walletTo.address, 7);
  });

  it('Can not transfer above the amount', async () => {
    await expect(token.transfer(walletTo.address, 1007)).to.be.reverted;
  });

  it('Can not transfer from empty account', async () => {
    const tokenFromOtherWallet = token.connect(walletTo);
    await expect(tokenFromOtherWallet.transfer(wallet.address, 1))
      .to.be.reverted;
  });

  it('Calls totalSupply on BasicToken contract', async () => {
    await token.totalSupply();
    expect('totalSupply').to.be.calledOnContract(token);
  });

  it('Calls balanceOf with sender address on BasicToken contract', async () => {
    await token.balanceOf(wallet.address);
    expect('balanceOf').to.be.calledOnContractWith(token, [wallet.address]);
  });
});
```

Note: You will also need to install following dependencies with to run the example above:

```sh
yarn add mocha -D
yarn add chai -D
```

Or with npm:

```
npm i chai -D
npm i mocha -D
```

### Compiling
To compile your smart contracts run:

```sh
npx waffle
```

To compile using a custom configuration file run:

```sh
npx waffle config.json
```

Example configuration file looks like this (all fields optional):
```json
{
  "sourceDirectory": "./custom_contracts",
  "outputDirectory": "./custom_build",
  "nodeModulesDirectory": "./custom_node_modules"
}
```

To enable generation of [typechain](https://github.com/ethereum-ts/TypeChain) artifacts:
```json
{
  "typechainEnabled": true
}
```

### Flattener
To flat your smart contracts run:

```sh
npx waffle flatten
```

In configuration file you can add optional field with path to flatten files:
```json
{
  "flattenOutputDirectory": "./custom_flatten"
}
```



### Running tests
To run the tests run the following command:

```sh
npx mocha
```

### Adding an npm script
For convinience, you can add the following to your `package.json`:
```
{
  ...,
  "scripts": {
    "test": "waffle && mocha"
  }
}
```

Now you can build and test your contracts with one command:

```sh
npm test
```

## Documentation
For detailed feature walkthrough checkout [documentation](https://ethereum-waffle.readthedocs.io/en/latest/).

## Contributing

Contributions are always welcome, no matter how large or small. Before contributing, please read the [code of conduct](https://github.com/EthWorks/Waffle/blob/master/CODE_OF_CONDUCT.md) and [contribution policy](https://github.com/EthWorks/Waffle/blob/master/CONTRIBUTION.md).

Before you issue pull request:

Make sure all tests and linters pass.
Make sure you have test coverage for any new features.

### Running tests
Note: To make end-to-end test pass, you need to:
* have Docker installed, up and running
* have Ethereum stable docker image pulled, if not run `docker pull ethereum/solc:stable`
* have native solidity 0.5.* installed

To run tests type:
```sh
yarn test
```

To run linter type:
```sh
yarn lint
```

### Building documentation

[Install Sphinx](https://www.sphinx-doc.org/en/master/usage/installation.html) to build documentation:

```sh
cd docs
make html
```

Before building documentation for the first time you may have to install required python packages:
```sh
pip3 install -r docs/requirements.txt
```

## Roadmap

See https://github.com/EthWorks/Waffle/issues/155

## License

Waffle is released under the [MIT License](https://opensource.org/licenses/MIT).
