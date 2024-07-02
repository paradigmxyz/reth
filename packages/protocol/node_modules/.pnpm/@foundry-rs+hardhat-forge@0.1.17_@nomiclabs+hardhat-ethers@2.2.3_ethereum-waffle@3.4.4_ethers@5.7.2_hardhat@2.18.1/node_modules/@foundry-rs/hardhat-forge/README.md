# hardhat-forge

This Hardhat plugin is for [forge](https://github.com/foundry-rs/foundry/tree/master/forge).

## What

This plugin provides bindings for `forge` commands as hardhat tasks.

## Installation

```bash
npm install --save-dev @foundry-rs/hardhat-forge
```

And add the following statement to your `hardhat.config.js`:

```js
require("@foundry-rs/hardhat-forge");
```

Or, if you are using TypeScript, add this to your `hardhat.config.ts`:

```js
import "@foundry-rs/hardhat-forge";
```

## Tasks

This plugin provides the following tasks:

- "forge::build" for "`forge build`
- "forge::test" for "`forge test`

## Configuration

See [Foundry](https://github.com/foundry-rs/foundry).

## LICENSE

- MIT license ([LICENSE](LICENSE) or https://opensource.org/licenses/MIT)
