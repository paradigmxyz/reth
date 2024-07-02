# `solidity-docgen`

*solidity-docgen is a program that extracts documentation for a Solidity project.*

The output is fully configurable through Handlebars templates, but the default templates should do a good job of displaying all of the information in the source code in Markdown format. The generated Markdown files can be used with a static site generator such as Vuepress, MkDocs, Jekyll (GitHub Pages), etc., in order to publish a documentation website.

This is a newer version of the tool that has been rewritten and redesigned. Some more work is pending to ease the transition from the previous version and to help with usage and configuration.

## Usage

Install `solidity-docgen` from npm.

### Hardhat

Include the plugin in your Hardhat configuration.

```diff
 // hardhat.config.ts
+import 'solidity-docgen';

 export default {
+  docgen: { ... }, // if necessary to customize config
 };
```

Then run with `hardhat docgen`.

### As a library

```typescript
import { docgen } from 'solidity-docgen';

await docgen([{ output: solcOutput }], config);
```

`solcOutput` must be the standard JSON output of the compiler, with at least the `ast` output. There can be more than one.

`config` is an optional object with the values as specified below.

## Config

See [`config.ts`](./src/config.ts) for the list of options and their documentation.
