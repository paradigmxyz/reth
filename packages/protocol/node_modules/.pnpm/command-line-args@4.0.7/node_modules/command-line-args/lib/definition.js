'use strict'
const t = require('typical')

/**
 * @module definition
 */

/**
 * Describes a command-line option. Additionally, you can add `description` and `typeLabel` properties and make use of [command-line-usage](https://github.com/75lb/command-line-usage).
 * @alias module:definition
 * @typicalname option
 */
class OptionDefinition {
  constructor (definition) {
    /**
    * The only required definition property is `name`, so the simplest working example is
    * ```js
    * [
    *   { name: "file" },
    *   { name: "verbose" },
    *   { name: "depth"}
    * ]
    * ```
    *
    * In this case, the value of each option will be either a Boolean or string.
    *
    * | #   | Command line args | .parse() output |
    * | --- | -------------------- | ------------ |
    * | 1   | `--file` | `{ file: true }` |
    * | 2   | `--file lib.js --verbose` | `{ file: "lib.js", verbose: true }` |
    * | 3   | `--verbose very` | `{ verbose: "very" }` |
    * | 4   | `--depth 2` | `{ depth: "2" }` |
    *
    * Unicode option names and aliases are valid, for example:
    * ```js
    * [
    *   { name: 'один' },
    *   { name: '两' },
    *   { name: 'три', alias: 'т' }
    * ]
    * ```
    * @type {string}
    */
    this.name = definition.name

    /**
    * The `type` value is a setter function (you receive the output from this), enabling you to be specific about the type and value received.
    *
    * You can use a class, if you like:
    *
    * ```js
    * const fs = require('fs')
    *
    * function FileDetails(filename){
    *   if (!(this instanceof FileDetails)) return new FileDetails(filename)
    *   this.filename = filename
    *   this.exists = fs.existsSync(filename)
    * }
    *
    * const cli = commandLineArgs([
    *   { name: 'file', type: FileDetails },
    *   { name: 'depth', type: Number }
    * ])
    * ```
    *
    * | #   | Command line args| .parse() output |
    * | --- | ----------------- | ------------ |
    * | 1   | `--file asdf.txt` | `{ file: { filename: 'asdf.txt', exists: false } }` |
    *
    * The `--depth` option expects a `Number`. If no value was set, you will receive `null`.
    *
    * | #   | Command line args | .parse() output |
    * | --- | ----------------- | ------------ |
    * | 2   | `--depth` | `{ depth: null }` |
    * | 3   | `--depth 2` | `{ depth: 2 }` |
    *
    * @type {function}
    * @default String
    */
    this.type = definition.type || String

    /**
    * getopt-style short option names. Can be any single character (unicode included) except a digit or hypen.
    *
    * ```js
    * [
    *   { name: "hot", alias: "h", type: Boolean },
    *   { name: "discount", alias: "d", type: Boolean },
    *   { name: "courses", alias: "c" , type: Number }
    * ]
    * ```
    *
    * | #   | Command line | .parse() output |
    * | --- | ------------ | ------------ |
    * | 1   | `-hcd` | `{ hot: true, courses: null, discount: true }` |
    * | 2   | `-hdc 3` | `{ hot: true, discount: true, courses: 3 }` |
    *
    * @type {string}
    */
    this.alias = definition.alias

    /**
    * Set this flag if the option takes a list of values. You will receive an array of values, each passed through the `type` function (if specified).
    *
    * ```js
    * [
    *   { name: "files", type: String, multiple: true }
    * ]
    * ```
    *
    * | #   | Command line | .parse() output |
    * | --- | ------------ | ------------ |
    * | 1   | `--files one.js two.js` | `{ files: [ 'one.js', 'two.js' ] }` |
    * | 2   | `--files one.js --files two.js` | `{ files: [ 'one.js', 'two.js' ] }` |
    * | 3   | `--files *` | `{ files: [ 'one.js', 'two.js' ] }` |
    *
    * @type {boolean}
    */
    this.multiple = definition.multiple

    /**
    * Any unclaimed command-line args will be set on this option. This flag is typically set on the most commonly-used option to make for more concise usage (i.e. `$ myapp *.js` instead of `$ myapp --files *.js`).
    *
    * ```js
    * [
    *   { name: "files", type: String, multiple: true, defaultOption: true }
    * ]
    * ```
    *
    * | #   | Command line | .parse() output |
    * | --- | ------------ | ------------ |
    * | 1   | `--files one.js two.js` | `{ files: [ 'one.js', 'two.js' ] }` |
    * | 2   | `one.js two.js` | `{ files: [ 'one.js', 'two.js' ] }` |
    * | 3   | `*` | `{ files: [ 'one.js', 'two.js' ] }` |
    *
    * @type {boolean}
    */
    this.defaultOption = definition.defaultOption

    /**
    * An initial value for the option.
    *
    * ```js
    * [
    *   { name: "files", type: String, multiple: true, defaultValue: [ "one.js" ] },
    *   { name: "max", type: Number, defaultValue: 3 }
    * ]
    * ```
    *
    * | #   | Command line | .parse() output |
    * | --- | ------------ | ------------ |
    * | 1   |  | `{ files: [ 'one.js' ], max: 3 }` |
    * | 2   | `--files two.js` | `{ files: [ 'two.js' ], max: 3 }` |
    * | 3   | `--max 4` | `{ files: [ 'one.js' ], max: 4 }` |
    *
    * @type {*}
    */
    this.defaultValue = definition.defaultValue

    /**
    * When your app has a large amount of options it makes sense to organise them in groups.
    *
    * There are two automatic groups: `_all` (contains all options) and `_none` (contains options without a `group` specified in their definition).
    *
    * ```js
    * [
    *   { name: "verbose", group: "standard" },
    *   { name: "help", group: [ "standard", "main" ] },
    *   { name: "compress", group: [ "server", "main" ] },
    *   { name: "static", group: "server" },
    *   { name: "debug" }
    * ]
    * ```
    *
    *<table>
    *  <tr>
    *    <th>#</th><th>Command Line</th><th>.parse() output</th>
    *  </tr>
    *  <tr>
    *    <td>1</td><td><code>--verbose</code></td><td><pre><code>
    *{
    *  _all: { verbose: true },
    *  standard: { verbose: true }
    *}
    *</code></pre></td>
    *  </tr>
    *  <tr>
    *    <td>2</td><td><code>--debug</code></td><td><pre><code>
    *{
    *  _all: { debug: true },
    *  _none: { debug: true }
    *}
    *</code></pre></td>
    *  </tr>
    *  <tr>
    *    <td>3</td><td><code>--verbose --debug --compress</code></td><td><pre><code>
    *{
    *  _all: {
    *    verbose: true,
    *    debug: true,
    *    compress: true
    *  },
    *  standard: { verbose: true },
    *  server: { compress: true },
    *  main: { compress: true },
    *  _none: { debug: true }
    *}
    *</code></pre></td>
    *  </tr>
    *  <tr>
    *    <td>4</td><td><code>--compress</code></td><td><pre><code>
    *{
    *  _all: { compress: true },
    *  server: { compress: true },
    *  main: { compress: true }
    *}
    *</code></pre></td>
    *  </tr>
    *</table>
    *
    * @type {string|string[]}
    */
    this.group = definition.group

    /* pick up any remaining properties */
    for (let prop in definition) {
      if (!this[prop]) this[prop] = definition[prop]
    }
  }

  isBoolean (value) {
    return this.type === Boolean || (t.isFunction(this.type) && this.type.name === 'Boolean')
  }
}

module.exports = OptionDefinition
