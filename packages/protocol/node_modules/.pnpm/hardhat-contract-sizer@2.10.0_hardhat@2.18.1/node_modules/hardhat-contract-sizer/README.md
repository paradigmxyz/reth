# Hardhat Contract Sizer

Output Solidity contract sizes with Hardhat.

> Versions of this plugin prior to `2.0.0` were released as `buidler-contract-sizer`.

## Installation

```bash
npm install --save-dev hardhat-contract-sizer
# or
yarn add --dev hardhat-contract-sizer
```

## Usage

Load plugin in Hardhat config:

```javascript
require("hardhat-contract-sizer");
```

Add configuration under the `contractSizer` key:

| option              | description                                                                                                                 | default |
| ------------------- | --------------------------------------------------------------------------------------------------------------------------- | ------- |
| `alphaSort`         | whether to sort results table alphabetically (default sort is by contract size)                                             | `false` |
| `runOnCompile`      | whether to output contract sizes automatically after compilation                                                            | `false` |
| `disambiguatePaths` | whether to output the full path to the compilation artifact (relative to the Hardhat root directory)                        | `false` |
| `strict`            | whether to throw an error if any contracts exceed the size limit (may cause compatibility issues with `solidity-coverage`)  | `false` |
| `only`              | `Array` of `String` matchers used to select included contracts, defaults to all contracts if `length` is 0                  | `[]`    |
| `except`            | `Array` of `String` matchers used to exclude contracts                                                                      | `[]`    |
| `outputFile`        | file path to write contract size report                                                                                     | `null`  |
| `unit`              | unit of measurement for the size of contracts, which can be expressed in 'B' (bytes), 'kB' (kilobytes) or 'KiB' (kibibytes) | `KiB`   |

```javascript
contractSizer: {
  alphaSort: true,
  disambiguatePaths: false,
  runOnCompile: true,
  strict: true,
  only: [':ERC20$'],
}
```

Run the included Hardhat task to output compiled contract sizes:

```bash
npx hardhat size-contracts
# or
yarn run hardhat size-contracts
```

By default, the hardhat `compile` task is run before sizing contracts. This behavior can be disabled with the `--no-compile` flag:

```bash
npx hardhat size-contracts --no-compile
# or
yarn run hardhat size-contracts --no-compile
```
