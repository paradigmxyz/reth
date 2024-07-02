[![CircleCI](https://circleci.com/gh/EthWorks/Waffle.svg?style=svg)](https://circleci.com/gh/EthWorks/Waffle)
[![](https://img.shields.io/npm/v/@ethereum-waffle/provider.svg)](https://www.npmjs.com/package/@ethereum-waffle/provider)

![Ethereum Waffle](https://raw.githubusercontent.com/EthWorks/Waffle/master/docs/source/logo.png)

# @ethereum-waffle/provider

A mock provider for your blockchain testing needs.

## Installation

In the current version of waffle (v2.x.x) you will install this package as a dependency of the main waffle package - `ethereum-waffle`.

```
yarn add --dev ethereum-waffle
npm install --save-dev ethereum-waffle
```

If you want to use this package directly please install it via:
```
yarn add --dev @ethereum-waffle/provider
npm install --save-dev @ethereum-waffle/provider
```

## Feature overview

**NOTE**: You do not need to use this package directly. You can install it through the main package (`ethereum-waffle`) and use it instead.

### MockProvider

The `MockProvider` class is the main way of interacting with the blockchain in your waffle tests.

It wraps `ganache-core` in an `ethers` provider and extends it with useful functionality.

You can learn more about it [in the documentation](https://ethereum-waffle.readthedocs.io/en/latest/basic-testing.html).

### Usage

Examples of creating `MockProvider` and usage `getWallets` function:
```ts
import {expect} from 'chai';
import {MockProvider} from '@ethereum-waffle/provider';

describe('waffle tests', async () => {
  it('wallets have non-zero balance', async () => {
    const provider = new MockProvider();
    const wallets = provider.getWallets();
    const balance = await wallets[0].getBalance();
    expect(balance.gt(0)).to.equal(true)
  })
})
```

Use `setupENS` function to set up your `ens` and setting ens address to the provider's networks. The function accepts
the `wallet` as an argument, and if you call it without an argument, it will use the last `wallet` from `MockProvider`.

```ts
import {expect} from 'chai';
import {MockProvider} from '@ethereum-waffle/provider';
describe('waffle tests', async () => {
  it('setup ens', async () => {
    const provider = new MockProvider();
    await provider.setupENS();
    expect(provider.network.ensAddress).to.eq(provider.ens.ens.address);
  });
});
```


### Fixtures

Fixtures are an advanced concept that you can use to make your tests run faster. They take advantage of the snapshot mechanism in Ganache.

You can learn more about it [in the documentation](https://ethereum-waffle.readthedocs.io/en/latest/fixtures.html).

### Legacy API

- `createMockProvider` - this was the old way to construct a MockProvider instance
- `getGanacheOptions` - previously it was possible to load the options from the waffle config file
- `getWallets` - this was the old way to get wallets from a provider
