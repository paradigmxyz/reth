<p align="center">
  <img src="solhint.png">
</p>
<p align="center">
  By <a href="https://protofire.io/">Protofire</a>
</p>

[![Donate with Ethereum](https://img.shields.io/badge/Donate-ETH-blue)](https://etherscan.io/address/0xA81705c8C247C413a19A244938ae7f4A0393944e)
[![NPM version](https://badge.fury.io/js/solhint.svg)](https://npmjs.org/package/solhint)
[![Coverage Status](https://coveralls.io/repos/github/protofire/solhint/badge.svg?branch=master)](
https://coveralls.io/github/protofire/solhint?branch=master)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://raw.githubusercontent.com/protofire/solhint/master/LICENSE)

This is an open source project for linting [Solidity](http://solidity.readthedocs.io/en/develop/) code. This project
provides both **Security** and **Style Guide** validations.

## Installation

You can install Solhint using **npm**:

```sh
npm install -g solhint

# verify that it was installed correctly
solhint --version
```

## Usage

First initialize a configuration file, if you don't have one:

```sh
solhint --init
```

This will create a `.solhint.json` file with the default rules enabled. Then run Solhint with one or more [Globs](https://en.wikipedia.org/wiki/Glob_(programming)) as arguments. For example, to lint all files inside `contracts` directory, you can do:

```sh
solhint 'contracts/**/*.sol'
```

To lint a single file:

```sh
solhint contracts/MyToken.sol
```

Run `solhint` without arguments to get more information:

```text
Usage: solhint [options] <file> [...other_files]

Linter for Solidity programming language

Options:

  -V, --version                           output the version number
  -f, --formatter [name]                  report formatter name (stylish, table, tap, unix, json, compact)
  -w, --max-warnings [maxWarningsNumber]  number of allowed warnings
  -c, --config [file_name]                file to use as your .solhint.json
  -q, --quiet                             report errors only - default: false
  --ignore-path [file_name]               file to use as your .solhintignore
  --fix                                   automatically fix problems
  --init                                  create configuration file for solhint
  -h, --help                              output usage information

Commands:

  stdin [options]                         linting of source code data provided to STDIN
  list-rules                              display covered rules of current .solhint.json
```
### Note
The `--fix`  option currently works only on "avoid-throw" and "avoid-sha3" rules
<br><br>
## Configuration

You can use a `.solhint.json` file to configure Solhint for the whole project.

To generate a new  sample `.solhint.json` file in current folder you can do:

```sh
solhint --init 
```

This file has the following format:
### Default 
```json
{
  "extends": "solhint:default"
}
```
### Note
The `solhint:default` configuration contains only two rules: max-line-length & no-console
<br><br>


### Sample
```json
  {
    "extends": "solhint:recommended",
    "plugins": [],
    "rules": {
      "avoid-suicide": "error",
      "avoid-sha3": "warn"
    }
  }
```
A full list of all supported rules can be found [here](docs/rules.md).

To ignore files that do not require validation you can use a `.solhintignore` file. It supports rules in
the `.gitignore` format.

```
node_modules/
additional-tests.sol
```

### Extendable rulesets

The default rulesets provided by solhint are the following:

+ solhint:default
+ solhint:recommended

Use one of these as the value for the "extends" property in your configuration file.

### Configure the linter with comments

You can use comments in the source code to configure solhint in a given line or file.

For example, to disable all validations in the line following a comment:

```solidity
  // solhint-disable-next-line
  uint[] a;
```

You can disable specific rules on a given line. For example:

```solidity
  // solhint-disable-next-line not-rely-on-time, not-rely-on-block-hash
  uint pseudoRand = uint(keccak256(abi.encodePacked(now, blockhash(block.number))));
```

Disable validation on current line:

```solidity
  uint pseudoRand = uint(keccak256(abi.encodePacked(now, blockhash(block.number)))); // solhint-disable-line
```

Disable specific rules on current line:

```solidity
   uint pseudoRand = uint(keccak256(abi.encodePacked(now, blockhash(block.number)))); // solhint-disable-line not-rely-on-time, not-rely-on-block-hash
```

You can disable a rule for a group of lines:

```solidity
  /* solhint-disable avoid-tx-origin */
  function transferTo(address to, uint amount) public {
    require(tx.origin == owner);
    to.call.value(amount)();
  }
  /* solhint-enable avoid-tx-origin */
```

Or disable all validations for a group of lines:

```solidity
  /* solhint-disable */
  function transferTo(address to, uint amount) public {
    require(tx.origin == owner);
    to.call.value(amount)();
  }
  /* solhint-enable */
```

## Rules
### Security Rules
[Full list with all supported Security Rules](docs/rules.md#security-rules)
### Style Guide Rules
[Full list with all supported Style Guide Rules](docs/rules.md#style-guide-rules)
### Best Practices Rules
[Full list with all supported Best Practices Rules](docs/rules.md#best-practise-rules)

## Documentation

Related documentation you may find [here](https://protofire.github.io/solhint/).

## IDE Integrations

  - **[Sublime Text 3](https://packagecontrol.io/search/solhint)**
  - **[Atom](https://atom.io/packages/atom-solidity-linter)**
  - **[Vim](https://github.com/sohkai/syntastic-local-solhint)**
  - **[JetBrains IDEA, WebStorm, CLion, etc.](https://plugins.jetbrains.com/plugin/10177-solidity-solhint)**
  - **[VS Code: Solidity by Juan Blanco](
         https://marketplace.visualstudio.com/items?itemName=JuanBlanco.solidity)**
  - **[VS Code: Solidity Language Support by CodeChain.io](
         https://marketplace.visualstudio.com/items?itemName=kodebox.solidity-language-server)**

## Table of Contents

* [Roadmap](ROADMAP.md): The core project's roadmap - what the core team is looking to work on in the near future.
* [Contributing](docs/contributing.md): The core Solhint team :heart: contributions. This describes how you can contribute to the Solhint Project.
* [Shareable configs](docs/shareable-configs.md): How to create and share your own configurations.
* [Writing plugins](docs/writing-plugins.md): How to extend Solhint with your own rules.

## Plugins

- [solhint-plugin-prettier](https://github.com/fvictorio/solhint-plugin-prettier): Integrate Solhint
  with the [Solidity plugin for Prettier](https://github.com/prettier-solidity/prettier-plugin-solidity).

## Who uses Solhint?
[<img src="https://avatars0.githubusercontent.com/u/20820676?s=200&v=4" width="75px" height="75px" alt="OpenZeppelin" title="OpenZeppelin" style="margin: 20px 20px 0 0" />](https://github.com/OpenZeppelin)
[<img src="https://avatars2.githubusercontent.com/u/28943015?s=200&v=4" width="75px" height="75px" alt="POA Network - Public EVM Sidechain" title="POA Network - Public EVM Sidechain" style="margin: 20px 20px 0 0" />](https://github.com/poanetwork) [<img src="https://avatars3.githubusercontent.com/u/24832717?s=200&v=4" width="75px" height="75px" alt="0x" title="0x" style="margin: 20px 20px 0 0" />](https://github.com/0xProject) [<img src="https://avatars1.githubusercontent.com/u/24954468?s=200&v=4" width="75px" height="75px" alt="GNOSIS" title="GNOSIS" style="margin: 20px 20px 0 0"/>](https://github.com/gnosis)

### Projects

- OpenZeppelin:
  - [openzeppelin-contracts](https://github.com/OpenZeppelin/openzeppelin-contracts)
- POA Network - Public EVM Sidechain:
  - [Proof of Physical Address (PoPA)](https://github.com/poanetwork/poa-popa)
  - [Proof of Bank Account (PoBA)](https://github.com/poanetwork/poa-poba)
- 0x-Project
  - [0x-Monorepo](https://github.com/0xProject/0x-monorepo/tree/development/contracts)
- Gnosis:
  - [Gnosis Prediction Market Contracts](https://github.com/gnosis/pm-contracts)
  - [The DutchX decentralized trading protocol](https://github.com/gnosis/dex-contracts)

## Acknowledgements

The Solidity parser used is [`@solidity-parser/parser`](https://github.com/solidity-parser/parser).

## Licence

MIT

## Back us
Solhint is free to use and open-sourced. If you value our effort and feel like helping us to keep pushing this tool forward, you can send us a small donation. We'll highly appreciate it :)

[![Donate with Ethereum](https://img.shields.io/badge/Donate-ETH-blue)](https://etherscan.io/address/0xA81705c8C247C413a19A244938ae7f4A0393944e)

## Related projects

- [eth-cli](https://github.com/protofire/eth-cli): CLI swiss army knife for Ethereum developers.
