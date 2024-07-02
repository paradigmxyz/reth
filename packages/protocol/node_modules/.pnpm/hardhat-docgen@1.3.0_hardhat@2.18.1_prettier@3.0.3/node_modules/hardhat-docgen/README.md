# Hardhat Docgen

Generate a static documentation site from NatSpec comments automatically on compilation with Hardhat.

## Installation

```bash
yarn add --dev hardhat-docgen
```

## Usage

Load plugin in Hardhat config:

```javascript
require('hardhat-docgen');
```

Add configuration under the `docgen` key:

| option | description | default |
|-|-|-|
| `path` | path to HTML export directory (relative to Hardhat root) | `'./docgen'`
| `clear` | whether to delete old files in `path` on documentation generation  | `false` |
| `runOnCompile` | whether to automatically generate documentation during compilation | `false` |
| `only` | `Array` of `String` matchers used to select included contracts, defaults to all contracts if `length` is 0 | `['^contracts/']` (dependent on Hardhat `paths` configuration) |
| `except` | `Array` of `String` matchers used to exclude contracts | `[]` |

```javascript
docgen: {
  path: './docs',
  clear: true,
  runOnCompile: true,
}
```

The included Hardhat task may be run manually:

```bash
yarn run hardhat docgen
```

By default, the hardhat `compile` task is run before generating documentation.  This behavior can be disabled with the `--no-compile` flag:

```bash
yarn run hardhat docgen --no-compile
```

The `path` directory will be created if it does not exist.

The `clear` option is set to `false` by default because it represents a destructive action, but should be set to `true` in most cases.
