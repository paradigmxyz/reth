<p align="center">
  <img src="https://github.com/Neufund/TypeChain/blob/d82f3cc644a11e22ca8e42505c16f035e2f2555d/docs/images/typechain-logo.png?raw=true" width="300" alt="TypeChain">
  <h3 align="center">TypeChain</h3>
  <p align="center">🔌 TypeScript bindings for Ethereum smartcontracts</p>

  <p align="center">
    <a href="https://circleci.com/gh/ethereum-ts/TypeChain"><img alt="Build Status" src="https://circleci.com/gh/ethereum-ts/TypeChain/tree/master.svg?style=svg"></a>
    <a href="https://coveralls.io/github/ethereum-ts/TypeChain?branch=master"><img alt="Coverage" src="https://coveralls.io/repos/github/ethereum-ts/TypeChain/badge.svg?branch=master"></a>
    <img alt="Downloads" src="https://img.shields.io/npm/dm/typechain.svg">
    <a href="https://github.com/prettier/prettier"><img alt="Prettier" src="https://img.shields.io/badge/code_style-prettier-ff69b4.svg"></a>
    <a href="/package.json"><img alt="Software License" src="https://img.shields.io/badge/license-MIT-brightgreen.svg?style=flat-square"></a>
  </p>

  <p align="center">
    <a href="https://blog.neufund.org/introducing-typechain-typescript-bindings-for-ethereum-smart-contracts-839fc2becf22">Medium post</a> | <a href="https://www.youtube.com/watch?v=9x6AkShGkwU">DappCon Video</a>
  </p>

  <p align="center">
    Contributed with: <br/>
    <img src="https://github.com/Neufund/TypeChain/blob/6d358df7b2da6b62d56f9935f1666b17b93176f0/docs/images/neufund-logo.png?raw=true" width="100" alt="Neufund">
  </p>
</p>

## Features ⚡

- static typing - you will never call not existing method again
- IDE support - works with any IDE supporting Typescript
- extendible - work with many different APIs: `ethers.js v4`, `truffle v4`, `truffle v5`, `Web3.js v1`, `Web3.js v2` or
  you can create your own target
- frictionless - works with simple, JSON ABI files as well as with Truffle style ABIs

## Installation

```bash
npm install --save-dev typechain
```

You will also need to install a desired target for example `@typechain/ethers-v4`. [Learn more about targets](#targets-)

## Packages 📦

| Package                                                | Version                                                                                                               | Description           | Examples                         |
| ------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------- | --------------------- | -------------------------------- |
| [`typechain`](/packages/typechain)                     | [![npm](https://img.shields.io/npm/v/typechain.svg)](https://www.npmjs.com/package/typechain)                         | Core package          | -                                |
| [`@typechain/ethers-v5`](/packages/target-ethers-v5)   | [![npm](https://img.shields.io/npm/v/@typechain/ethers-v5.svg)](https://www.npmjs.com/package/@typechain/ethers-v5)   | Ethers ver 5 support  | [example](./examples/ethers-v5)  |
| [`@typechain/ethers-v4`](/packages/target-ethers-v4)   | [![npm](https://img.shields.io/npm/v/@typechain/ethers-v4.svg)](https://www.npmjs.com/package/@typechain/ethers-v4)   | Ethers ver 4 support  | [example](./examples/ethers-v4)  |
| [`@typechain/truffle-v5`](/packages/target-truffle-v5) | [![npm](https://img.shields.io/npm/v/@typechain/truffle-v5.svg)](https://www.npmjs.com/package/@typechain/truffle-v5) | Truffle ver 5 support | [example](./examples/truffle-v5) |
| [`@typechain/truffle-v4`](/packages/target-truffle-v4) | [![npm](https://img.shields.io/npm/v/@typechain/truffle-v4.svg)](https://www.npmjs.com/package/@typechain/truffle-v4) | Truffle ver 4 support | [example](./examples/truffle-v4) |
| [`@typechain/web3-v1`](/packages/target-web3-v1)       | [![npm](https://img.shields.io/npm/v/@typechain/web3-v1.svg)](https://www.npmjs.com/package/@typechain/web3-v1)       | Web3 ver 1 support    | [example](./examples/web3-v1)    |

## Usage

### CLI

```
typechain --target=(ethers-v4|truffle-v4|truffle-v5|web3-v1|path-to-custom-target) [glob]
```

- `glob` - pattern that will be used to find ABIs, remember about adding quotes: `typechain "**/*.json"`, examples:
  `./abis/**/*.abi`, `./abis/?(Oasis.abi|OasisHelper.abi)`.
- `--target` - ethers-v4, truffle-v4, truffle-v5, web3-v1 or path to your custom target. Typechain will try to load
  package named: `@typechain/${target}`, so make sure that desired package is installed.
- `--outDir` (optional) - put all generated files to a specific dir.

TypeChain always will rewrite existing files. You should not commit them. Read more in FAQ section.

Example:

```
typechain --target ethers-v4 --outDir app/contracts './node_modules/neufund-contracts/build/contracts/*.json'
```

## Demo 🏎️

![Demo](https://media.giphy.com/media/3oFzmqgHxrPZFhBst2/giphy.gif)

[Example usage](https://github.com/Neufund/commit.neufund.org/pull/331/files)

## Getting started 📚

### Motivation

Interacting with blockchain in Javascript is a pain. Web3 interface is sluggish and when using it with Typescript it
gets even worse. Often, you can't be sure what given method call will actually do without looking at ABI file. TypeChain
is here to solve these problems (as long as you use Typescript).

### How does it work?

TypeChain is code generator - provide ABI file and you will get Typescript class with flexible interface for interacting
with blockchain. Depending on the target parameter it can generate typings for truffle, web3 1.0.0 or ethers.

### Step by step guide

Install typechain with `yarn add --dev typechain` and install desired target.

Run `typechain --target=your_target` (you might need to make sure that it's available in your path if you installed it
only locally), it will automatically find all `.abi` files in your project and generate Typescript classes based on
them. You can specify your glob pattern: `typechain --target=your_target "**/*.abi.json"`. `node_modules` are always
ignored. We recommend git ignoring these generated files and making typechain part of your build process.

That's it! Now, you can simply import typings, check out our examples for more details.

## Targets 🎯

### Ethers.js v4 / v5

Use `ethers-v4` target to generate wrappers for [ethers.js](https://github.com/ethers-io/ethers.js/) lib.

### Truffle v4 / v5

Truffle target is great when you use truffle contracts already. Check out
[truffle-typechain-example](https://github.com/ethereum-ts/truffle-typechain-example) for more details. It require
installing [typings](https://www.npmjs.com/package/truffle-typings) for truffle library itself.

Now you can simply use your contracts as you did before and get full type safety, yay!

### Web3 v1

Generates typings for contracts compatible with latest stable Web3.js version. Typings for library itself are now part
of the `Web3 1.0.0` library so nothing additional is needed. For now it needs explicit cast as shown
[here](https://github.com/krzkaczor/TypeChain/pull/88/files#diff-540a9b8840419be93ddb8d4b53325637R8), this will be fixed
after improving official typings.

### NatSpec support

If you provide solidity artifacts rather than plain ABIs as an input, TypeChain can generate NatSpec comments directly
to your typings which enables simple access to docs while coding.

### Your own target

This might be useful when you're creating a library for users of your smartcontract and you don't want to lock yourself
into any API provided by Web3 access providing library. You can generate basically any code (even for different
languages than TypeScript!) that based on smartcontract's ABI.

## FAQ 🤔

### Q: Should I commit generated files to my repository?

A: _NO_ — we believe that no generated files should go to git repository. You should git ignore them and make
`typechain` run automatically for example in post install hook in package.json:

```
"postinstall":"typechain"
```

When you update ABI, just regenerate files with TypeChain and Typescript compiler will find any breaking changes for
you.

### Q: How do I customize generated code?

A: You can create your own target and generate basically any code.

### Q: Generated files won't match current codestyle of my project :(

A: We will automatically format generated classes with `prettier` to match your coding preferences (just make sure to
use `.prettierrc` file).

Furthermore, we will silent tslint for generated files with `/* tslint:disable */` comments.

### Usage as API

You may want to use `ts-generator` api to kick off whole process by api:

```typescript
import { tsGenerator } from 'ts-generator'
import { TypeChain } from 'typechain/dist/TypeChain'

async function main() {
  const cwd = process.cwd()

  await tsGenerator(
    { cwd },
    new TypeChain({
      cwd,
      rawConfig: {
        files: 'your-glob-here',
        outDir: 'optional out dir path',
        target: 'your-target',
      },
    }),
  )
}

main().catch(console.error)
```

# Contributing

Check out our [contributing guidelines](./CONTRIBUTING.md)

# Licence

Krzysztof Kaczor (krzkaczor) MIT | [Github](https://github.com/krzkaczor) | [Twitter](https://twitter.com/krzkaczor)
