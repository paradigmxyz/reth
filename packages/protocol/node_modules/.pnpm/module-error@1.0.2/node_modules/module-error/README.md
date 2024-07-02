# module-error

**Create errors with `code` and `cause` properties. Simple and extensible.**

[![npm status](http://img.shields.io/npm/v/module-error.svg)](https://www.npmjs.org/package/module-error)
[![node](https://img.shields.io/node/v/module-error.svg)](https://www.npmjs.org/package/module-error)
[![Test](https://img.shields.io/github/workflow/status/vweevers/module-error/Test?label=test)](https://github.com/vweevers/module-error/actions/workflows/test.yml)
[![Standard](https://img.shields.io/badge/standard-informational?logo=javascript&logoColor=fff)](https://standardjs.com)
[![Common Changelog](https://common-changelog.org/badge.svg)](https://common-changelog.org)

## Usage

Works like a regular `Error` constructor but adds an options argument (as [`proposal-error-cause`](https://github.com/tc39/proposal-error-cause) does).

```js
const ModuleError = require('module-error')

throw new ModuleError('Message goes here', {
  code: 'EXAMPLE_NOT_FOUND'
})
```

The primary purpose of `ModuleError` is to define a `code` property on the error object following Node.js conventions. It should be set to an uppercase string that uniquely identifies a situation, prefixed with the name of your module (or a collection of modules) to prevent conflicts.

The output looks like this in Node.js (some stack frames omitted for brevity):

```
ModuleError: Message goes here
    at Object.<anonymous> (/home/app/example.js:5:7)
    at node:internal/main/run_main_module:17:47 {
  code: 'EXAMPLE_NOT_FOUND'
}
```

The benefit of error codes is that messages can be changed without a semver-major release because your [semver](https://semver.org) contract will be on the codes. Codes can be reused across related modules while allowing individual modules to customize messages. I also prefer it over `instanceof MyError` logic because codes work cross-realm and when a tree of `node_modules` contains multiple versions of a module.

To wrap another error:

```js
try {
  JSON.parse(await fs.readFile('state.json'))
} catch (err) {
  throw new ModuleError('Could not load state', {
    code: 'EXAMPLE_INVALID_STATE',
    cause: err
  })
}
```

If for convenience you want to create subclasses with prepared codes:

```js
class NotFoundError extends ModuleError {
  constructor(message, options) {
    super(message, { ...options, code: 'EXAMPLE_NOT_FOUND' })
  }
}
```

Then you can do:

```js
throw new NotFoundError('Message goes here')
```

Under Node.js the stack trace will be adjusted accordingly, to skip the frame containing your error constructor.

## API

### `ModuleError(message, [options])`

Constructor to create an error with the provided `message` string. Options:

- `code` (string): if provided, define a `code` property with this value.
- `cause` (Error): if provided, define a `cause` property with this value. Unlike the spec of [`proposal-error-cause`](https://github.com/tc39/proposal-error-cause) the property is enumerable so that Node.js (v16 at the time of writing) will print it. Firefox prints it regardless, Chromium doesn't yet.
- `expected`: if truthy, define a `expected` property with value `true`. This is useful for command line interfaces to differentiate unexpected errors from e.g. invalid user input. A pattern I like to follow is to print only an error message if `err.expected` is true and no `--verbose` flag was provided. Otherwise print the full stack.
- `transient`: if truthy, define a `transient` property with value `true`. This communicates to users that the operation that caused the error may be retried. See also [`transient-error`](https://github.com/vweevers/transient-error) to mark existing errors as transient.

## Install

With [npm](https://npmjs.org) do:

```
npm install module-error
```

## License

[MIT](LICENSE)
