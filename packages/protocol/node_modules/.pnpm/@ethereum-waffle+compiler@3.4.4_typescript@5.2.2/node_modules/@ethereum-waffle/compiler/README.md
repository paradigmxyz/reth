[![CircleCI](https://circleci.com/gh/EthWorks/Waffle.svg?style=svg)](https://circleci.com/gh/EthWorks/Waffle)
[![](https://img.shields.io/npm/v/@ethereum-waffle/compiler.svg)](https://www.npmjs.com/package/@ethereum-waffle/compiler)

![Ethereum Waffle](https://raw.githubusercontent.com/EthWorks/Waffle/master/docs/source/logo.png)

# @ethereum-waffle/compiler

Compile solidity without the hassle.

## Installation

In the current version of waffle (v2.x.x) you will install this package as a dependency of the main waffle package - `ethereum-waffle`.

```
yarn add --dev ethereum-waffle
npm install --save-dev ethereum-waffle
```

If you want to use this package directly please install it via:
```
yarn add --dev @ethereum-waffle/compiler
npm install --save-dev @ethereum-waffle/compiler
```

## Feature overview

**NOTE**: You do not need to use this package directly. You can install it through the main package (`ethereum-waffle`) and use the CLI instead.

### Compilation

This package exposes programmatic api for compiling solidity smart contracts.

You can learn more about it [in the documentation](https://ethereum-waffle.readthedocs.io/en/latest/compilation.html).

Examples:
```ts
// Compilation with a config file
const {compileProject} = require('@ethereum-waffle/compiler');

main();
async function main () {
  await compileProject('path/to/waffle.json');
}
```

```ts
// Compilation with js config
const {compileAndSave, compile} = require('@ethereum-waffle/compiler');

main();
async function main () {
  const config = { sourceDirectory: 'contracts', nodeModulesDirectory: 'node_modules' };

  // compile and save the output
  await compileAndSave(config);

  // just compile
  const output = await compile(config);
  console.log(output);
}
```

### Linking

Example:
```ts
const {link} = require('@ethereum-waffle/compiler');
```
