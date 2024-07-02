[![CircleCI](https://circleci.com/gh/EthWorks/Waffle.svg?style=svg)](https://circleci.com/gh/EthWorks/Waffle)
[![](https://img.shields.io/npm/v/@ethereum-waffle/chai.svg)](https://www.npmjs.com/package/@ethereum-waffle/chai)

![Ethereum Waffle](https://raw.githubusercontent.com/EthWorks/Waffle/master/docs/source/logo.png)

# @ethereum-waffle/chai

A sweet set of chai matchers for your blockchain testing needs.

## Installation

In the current version of waffle (v2.x.x) you will install this package as a dependency of the main waffle package - `ethereum-waffle`.

```
yarn add --dev ethereum-waffle
npm install --save-dev ethereum-waffle
```

If you want to use this package directly please install it via:
```
yarn add --dev @ethereum-waffle/chai
npm install --save-dev @ethereum-waffle/chai
```

## Usage
```ts
import { expect, use } from "chai";
import { waffleChai } from "@ethereum-waffle/chai";
import { bigNumberify } from "ethers/utils";

use(chaiAsPromised);

expect(bigNumberify("6")).to.be.gt(0);
```

## Feature overview

**NOTE**: You do not need to use this package directly. You can install it through the main package (`ethereum-waffle`) and use it instead.

Read more [in the documentation](https://ethereum-waffle.readthedocs.io/en/latest/matchers.html).
