[![Build Status](https://img.shields.io/travis/ethereum/solc-js.svg?branch=master&style=flat-square)](https://travis-ci.org/ethereum/solc-js)
[![Coverage Status](https://img.shields.io/coveralls/ethereum/solc-js.svg?style=flat-square)](https://coveralls.io/r/ethereum/solc-js)

# solc-js
JavaScript bindings for the [Solidity compiler](https://github.com/ethereum/solidity).

Uses the Emscripten compiled Solidity found in the [solc-bin repository](https://github.com/ethereum/solc-bin).

## Node.js Usage

To use the latest stable version of the Solidity compiler via Node.js you can install it via npm:

```bash
npm install solc
```

### Usage on the Command-Line

If this package is installed globally (`npm install -g solc`), a command-line tool called `solcjs` will be available.

To see all the supported features, execute:

```bash
solcjs --help
```

Note: this commandline interface is not compatible with `solc` provided by the Solidity compiler package and thus cannot be
used in combination with an Ethereum client via the `eth.compile.solidity()` RPC method. Please refer to the
[Solidity compiler documentation](https://solidity.readthedocs.io/) for instructions to install `solc`.

### Usage in Projects

#### From early versions

It can also be included and used in other projects:

```javascript
var solc = require('solc')
var input = 'contract x { function g() {} }'
// Setting 1 as second paramateractivates the optimiser
var output = solc.compile(input, 1)
for (var contractName in output.contracts) {
	// code and ABI that are needed by web3
	console.log(contractName + ': ' + output.contracts[contractName].bytecode)
	console.log(contractName + '; ' + JSON.parse(output.contracts[contractName].interface))
}
```

#### From version 0.1.6

Starting from version 0.1.6, multiple files are supported with automatic import resolution by the compiler as follows:

```javascript
var solc = require('solc')
var input = {
	'lib.sol': 'library L { function f() returns (uint) { return 7; } }',
	'cont.sol': 'import "lib.sol"; contract x { function g() { L.f(); } }'
}
var output = solc.compile({ sources: input }, 1)
for (var contractName in output.contracts)
	console.log(contractName + ': ' + output.contracts[contractName].bytecode)
```

Note that all input files that are imported have to be supplied, the compiler will not load any additional files on its own.

#### From version 0.2.1

Starting from version 0.2.1, a callback is supported to resolve missing imports as follows:

```javascript
var solc = require('solc')
var input = {
	'cont.sol': 'import "lib.sol"; contract x { function g() { L.f(); } }'
}
function findImports (path) {
	if (path === 'lib.sol')
		return { contents: 'library L { function f() returns (uint) { return 7; } }' }
	else
		return { error: 'File not found' }
}
var output = solc.compile({ sources: input }, 1, findImports)
for (var contractName in output.contracts)
	console.log(contractName + ': ' + output.contracts[contractName].bytecode)
```

The `compile()` method always returns an object, which can contain `errors`, `sources` and `contracts` fields. `errors` is a list of error mesages.

#### From version 0.4.11

Starting from version 0.4.11 there is a new entry point named `compileStandardWrapper()` which supports Solidity's [standard JSON input and output](https://solidity.readthedocs.io/en/develop/using-the-compiler.html#compiler-input-and-output-json-description). It also maps old compiler output to it.

```javascript
var solc = require('solc')

// 'input' is a JSON string corresponding to the "standard JSON input" as described in the link above
// 'findImports' works as described above
var output = solc.compileStandardWrapper(input, findImports)
// Ouput is a JSON string corresponding to the "standard JSON output"
```

There is also a direct method, `compileStandard`, which is only present on recent compilers and works the same way. `compileStandardWrapper` is preferred however because it provides the same interface for old compilers.

#### From version 0.4.20

Starting from version 0.4.20 a Semver compatible version number can be retrieved on every compiler release, including old ones, using the `semver()` method.

### Using with Electron

**Note:**
If you are using Electron, `nodeIntegration` is on for `BrowserWindow` by default. If it is on, Electron will provide a `require` method which will not behave as expected and this may cause calls, such as `require('solc')`, to fail.

To turn off `nodeIntegration`, use the following:

```javascript
new BrowserWindow({
	webPreferences: {
		nodeIntegration: false
	}
})
```

### Using a Legacy Version

In order to compile contracts using a specific version of Solidity, the `solc.loadRemoteVersion(version, callback)` method is available. This returns a new `solc` object that uses a version of the compiler specified. 

You can also load the "binary" manually and use `setupMethods` to create the familiar wrapper functions described above:
`var solc = solc.setupMethods(require("/my/local/soljson.js"))`.

### Using the Latest Development Snapshot

By default, the npm version is only created for releases. This prevents people from deploying contracts with non-release versions because they are less stable and harder to verify. If you would like to use the latest development snapshot (at your own risk!), you may use the following example code.

```javascript
var solc = require('solc')

// getting the development snapshot
solc.loadRemoteVersion('latest', function (err, solcSnapshot) {
	if (err) {
		// An error was encountered, display and quit
	}
	var output = solcSnapshot.compile("contract t { function g() {} }", 1)
})
```

### Linking Bytecode

When using libraries, the resulting bytecode will contain placeholders for the real addresses of the referenced libraries. These have to be updated, via a process called linking, before deploying the contract.

The `linker` module (`require('solc/linker')`) offers helpers to accomplish this.

The `linkBytecode` method provides a simple helper for linking:

```javascript
var linker = require('solc/linker')

bytecode = linker.linkBytecode(bytecode, { 'MyLibrary': '0x123456...' })
```

(Note: `linkBytecode` is also exposed via `solc` as `solc.linkBytecode`, but this usage is deprecated.)

As of Solidity 0.4.11 the compiler supports [standard JSON input and output](https://solidity.readthedocs.io/en/develop/using-the-compiler.html#compiler-input-and-output-json-description) which outputs a *link references* map. This gives a map of library names to offsets in the bytecode to replace the addresses at. It also doesn't have the limitation on library file and contract name lengths.

There is a method available in the `linker` module called `findLinkReferences` which can find such link references in bytecode produced by an older compiler:

```javascript
var linker = require('solc/linker')

var linkReferences = linker.findLinkReferences(bytecode)
```

### Updating the ABI

The ABI generated by Solidity versions can differ slightly, due to new features introduced.  There is a tool included which aims to translate the ABI generated by an older Solidity version to conform to the latest standard.

It can be used as:
```javascript
var abi = require('solc/abi')

var inputABI = [{"constant":false,"inputs":[],"name":"hello","outputs":[{"name":"","type":"string"}],"payable":false,"type":"function"}]
var outputABI = abi.update('0.3.6', inputABI)
// Output contains: [{"constant":false,"inputs":[],"name":"hello","outputs":[{"name":"","type":"string"}],"payable":true,"type":"function"},{"type":"fallback","payable":true}]

```

### Formatting old JSON assembly output

There is a helper available to format old JSON assembly output into a text familiar to earlier users of Remix IDE.

```
var translate = require('solc/translate')

// assemblyJSON refers to the JSON of the given assembly and sourceCode is the source of which the assembly was generated from
var output = translate.prettyPrintLegacyAssemblyJSON(assemblyJSON, sourceCode)
```
