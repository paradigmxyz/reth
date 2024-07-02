[![CircleCI](https://circleci.com/gh/EthWorks/Waffle.svg?style=svg)](https://circleci.com/gh/EthWorks/Waffle)
[![](https://img.shields.io/npm/v/@ethereum-waffle/ens.svg)](https://www.npmjs.com/package/@ethereum-waffle/ens)

![Ethereum Waffle](https://raw.githubusercontent.com/EthWorks/Waffle/master/docs/source/logo.png)

# @ethereum-waffle/ens

A simple `ENS` for testing.

## Installation

In the current version of waffle (v2.x.x) you will install this package as a dependency of the main waffle package - `ethereum-waffle`.

```
yarn add --dev ethereum-waffle
npm install --save-dev ethereum-waffle
```

If you want to use this package directly, please install it via:
```
yarn add --dev @ethereum-waffle/ens
npm install --save-dev @ethereum-waffle/ens
```

## Feature overview

**NOTE**: You do not need to use this package directly. You can install it through the main package (`ethereum-waffle`) and use it instead.

### ENS

The `ENS` class allows to create ENS domains for testing.

It will deploy ENS smart contracts system to your test node.

To create `ENS`, you should submit your `wallet`, available in `MockProvider` class in package `@ethereum-waffle/provider`.

You can read more about ENS [in the ENS's documentation](https://docs.ens.domains/).

### Examples:
Creating ENS:
```ts
import {MockProvider} from '@ethereum-waffle/provider';
import {deployENS, ENS} from '@ethereum-waffle/ens';

const provider = new MockProvider();
const [wallet] = provider.getWallets();
const ens: ENS = await deployENS(wallet);
```

### Usage

Use `createTopLevelDomain` function to create a top level domain:

```ts
await ens.createTopLevelDomain('test');
```

Use `createSubDomain` function for creating a sub domain:

```ts
await ens.createSubDomain('ethworks.test');
```

Also, it's possible to create a sub domain recursively, if the top domain doesn't exist, by specifying the appropriate option:

```ts
await ens.createSubDomain('ethworks.tld', {recursive: true});
```

Use `setAddress` function for setting address for the domain:

```ts
await ens.setAddress('vlad.ethworks.test', '0x001...03');
```
Also, it's possible to set an address for the domain recursively, if the domain doesn't exist, by specifying the appropriate option:

```ts
await ens.setAddress('vlad.ethworks.tld', '0x001...03', {recursive: true});
```

Use `setAddressWithReverse` function for setting address for the domain and make this domain reverse. Add recursive option if the domain doesn't exist:

```ts
await ens.setAddressWithReverse('vlad.ethworks.tld', wallet, {recursive: true});
```
