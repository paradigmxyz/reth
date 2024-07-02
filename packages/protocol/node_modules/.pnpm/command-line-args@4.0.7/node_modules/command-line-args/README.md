[![view on npm](https://img.shields.io/npm/v/command-line-args.svg)](https://www.npmjs.org/package/command-line-args)
[![npm module downloads](https://img.shields.io/npm/dt/command-line-args.svg)](https://www.npmjs.org/package/command-line-args)
[![Build Status](https://travis-ci.org/75lb/command-line-args.svg?branch=master)](https://travis-ci.org/75lb/command-line-args)
[![Coverage Status](https://coveralls.io/repos/github/75lb/command-line-args/badge.svg?branch=master)](https://coveralls.io/github/75lb/command-line-args?branch=master)
[![Dependency Status](https://david-dm.org/75lb/command-line-args.svg)](https://david-dm.org/75lb/command-line-args)
[![js-standard-style](https://img.shields.io/badge/code%20style-standard-brightgreen.svg)](https://github.com/feross/standard)
[![Join the chat at https://gitter.im/75lb/command-line-args](https://badges.gitter.im/Join%20Chat.svg)](https://gitter.im/75lb/command-line-args?utm_source=badge&utm_medium=badge&utm_campaign=pr-badge&utm_content=badge)

# command-line-args
A mature, feature-complete library to parse command-line options.

*If your app requires a git-like command interface, consider using [command-line-commands](https://github.com/75lb/command-line-commands).*

## Synopsis
You can set options using the main notation standards (getopt, getopt_long, etc.). These commands are all equivalent, setting the same values:
```
$ example --verbose --timeout=1000 --src one.js --src two.js
$ example --verbose --timeout 1000 --src one.js two.js
$ example -vt 1000 --src one.js two.js
$ example -vt 1000 one.js two.js
```

To access the values, first describe the options your app accepts (see [option definitions](#optiondefinition-)).
```js
const commandLineArgs = require('command-line-args')

const optionDefinitions = [
  { name: 'verbose', alias: 'v', type: Boolean },
  { name: 'src', type: String, multiple: true, defaultOption: true },
  { name: 'timeout', alias: 't', type: Number }
]
```
The [`type`](#optiontype--function) property is a setter function (the value supplied is passed through this), giving you full control over the value received.

Next, parse the options using [commandLineArgs()](#commandlineargsdefinitions-argv--object-):
```js
const options = commandLineArgs(optionDefinitions)
```

`options` now looks like this:
```js
{
  files: [
    'one.js',
    'two.js'
  ],
  verbose: true,
  timeout: 1000
}
```

When dealing with large amounts of options it often makes sense to [group](#optiongroup--string--arraystring) them.

A usage guide can be generated using [command-line-usage](https://github.com/75lb/command-line-usage), for example:

![usage](https://raw.githubusercontent.com/75lb/command-line-usage/master/example/screens/footer.png)

### Notation rules

Notation rules for setting command-line options.

* Argument order is insignificant. Whether you set `--example` at the beginning or end of the arg list makes no difference.
* Options with a [type](#optiontype--function) of `Boolean` do not need to supply a value. Setting `--flag` or `-f` will set that option's value to `true`. This is the only [type](#optiontype--function) with special behaviour.
* Three ways to set an option value
  * `--option value`
  * `--option=value`
  * `-o value`
* Two ways to a set list of values (on options with [multiple](#optionmultiple--boolean) set)
  * `--list one two three`
  * `--list one --list two --list three`
* Short options ([alias](#optionalias--string)) can be set in groups. The following are equivalent:
  * `-a -b -c`
  * `-abc`

### Ambiguous values

Imagine we are using "grep-tool" to search for the string `'-f'`:

```
$ grep-tool --search -f
```

We have an issue here: command-line-args will assume we are setting two options (`--search` and `-f`). In actuality, we are passing one option (`--search`) and one value (`-f`). In cases like this, avoid ambiguity by using `--option=value` notation:

```
$ grep-tool --search=-f
```

### Partial parsing

By default, if the user sets an option without a valid [definition](#exp_module_definition--OptionDefinition) an `UNKNOWN_OPTION` exception is thrown. However, in some cases you may only be interested in a subset of the options wishing to pass the remainder to another library. See [here](https://github.com/75lb/command-line-args/blob/master/example/mocha.js) for a example showing where this might be necessary.

To enable partial parsing, set `partial: true` in the method options:

```js
const optionDefinitions = [
  { name: 'value', type: Number }
]
const options = commandLineArgs(optionDefinitions, { partial: true })
```

Now, should any unknown args be passed at the command line:

```
$ example --milk --value 2 --bread cheese
```

They will be returned in the `_unknown` property of the `commandLineArgs` output with no exceptions thrown:

```js
{
  value: 2,
  _unknown: [ '--milk', '--bread', 'cheese']
}
```


## Install

```sh
$ npm install command-line-args --save
```

# API Reference
<a name="exp_module_command-line-args--commandLineArgs"></a>

### commandLineArgs(optionDefinitions, [options]) ⇒ <code>object</code> ⏏
Returns an object containing all options set on the command line. By default it parses the global  [`process.argv`](https://nodejs.org/api/process.html#process_process_argv) array.

By default, an exception is thrown if the user sets an unknown option (one without a valid [definition](#exp_module_definition--OptionDefinition)). To enable __partial parsing__, invoke `commandLineArgs` with the `partial` option - all unknown arguments will be returned in the `_unknown` property.

**Kind**: Exported function  
**Throws**:

- `UNKNOWN_OPTION` if `options.partial` is false and the user set an undefined option
- `NAME_MISSING` if an option definition is missing the required `name` property
- `INVALID_TYPE` if an option definition has a `type` value that's not a function
- `INVALID_ALIAS` if an alias is numeric, a hyphen or a length other than 1
- `DUPLICATE_NAME` if an option definition name was used more than once
- `DUPLICATE_ALIAS` if an option definition alias was used more than once
- `DUPLICATE_DEFAULT_OPTION` if more than one option definition has `defaultOption: true`


| Param | Type | Description |
| --- | --- | --- |
| optionDefinitions | <code>[Array.&lt;definition&gt;](#module_definition)</code> | An array of [OptionDefinition](#exp_module_definition--OptionDefinition) objects |
| [options] | <code>object</code> | Options. |
| [options.argv] | <code>Array.&lt;string&gt;</code> | An array of strings, which if passed will be parsed instead  of `process.argv`. |
| [options.partial] | <code>boolean</code> | If `true`, an array of unknown arguments is returned in the `_unknown` property of the output. |

<a name="exp_module_definition--OptionDefinition"></a>

## OptionDefinition ⏏
Describes a command-line option. Additionally, you can add `description` and `typeLabel` properties and make use of [command-line-usage](https://github.com/75lb/command-line-usage).

**Kind**: Exported class  
* [OptionDefinition](#exp_module_definition--OptionDefinition) ⏏
    * [.name](#module_definition--OptionDefinition.OptionDefinition+name) : <code>string</code>
    * [.type](#module_definition--OptionDefinition.OptionDefinition+type) : <code>function</code>
    * [.alias](#module_definition--OptionDefinition.OptionDefinition+alias) : <code>string</code>
    * [.multiple](#module_definition--OptionDefinition.OptionDefinition+multiple) : <code>boolean</code>
    * [.defaultOption](#module_definition--OptionDefinition.OptionDefinition+defaultOption) : <code>boolean</code>
    * [.defaultValue](#module_definition--OptionDefinition.OptionDefinition+defaultValue) : <code>\*</code>
    * [.group](#module_definition--OptionDefinition.OptionDefinition+group) : <code>string</code> \| <code>Array.&lt;string&gt;</code>

<a name="module_definition--OptionDefinition.OptionDefinition+name"></a>

### option.name : <code>string</code>
The only required definition property is `name`, so the simplest working example is
```js
[
  { name: "file" },
  { name: "verbose" },
  { name: "depth"}
]
```

In this case, the value of each option will be either a Boolean or string.

| #   | Command line args | .parse() output |
| --- | -------------------- | ------------ |
| 1   | `--file` | `{ file: true }` |
| 2   | `--file lib.js --verbose` | `{ file: "lib.js", verbose: true }` |
| 3   | `--verbose very` | `{ verbose: "very" }` |
| 4   | `--depth 2` | `{ depth: "2" }` |

Unicode option names and aliases are valid, for example:
```js
[
  { name: 'один' },
  { name: '两' },
  { name: 'три', alias: 'т' }
]
```

**Kind**: instance property of <code>[OptionDefinition](#exp_module_definition--OptionDefinition)</code>  
<a name="module_definition--OptionDefinition.OptionDefinition+type"></a>

### option.type : <code>function</code>
The `type` value is a setter function (you receive the output from this), enabling you to be specific about the type and value received.

You can use a class, if you like:

```js
const fs = require('fs')

function FileDetails(filename){
  if (!(this instanceof FileDetails)) return new FileDetails(filename)
  this.filename = filename
  this.exists = fs.existsSync(filename)
}

const cli = commandLineArgs([
  { name: 'file', type: FileDetails },
  { name: 'depth', type: Number }
])
```

| #   | Command line args| .parse() output |
| --- | ----------------- | ------------ |
| 1   | `--file asdf.txt` | `{ file: { filename: 'asdf.txt', exists: false } }` |

The `--depth` option expects a `Number`. If no value was set, you will receive `null`.

| #   | Command line args | .parse() output |
| --- | ----------------- | ------------ |
| 2   | `--depth` | `{ depth: null }` |
| 3   | `--depth 2` | `{ depth: 2 }` |

**Kind**: instance property of <code>[OptionDefinition](#exp_module_definition--OptionDefinition)</code>  
**Default**: <code>String</code>  
<a name="module_definition--OptionDefinition.OptionDefinition+alias"></a>

### option.alias : <code>string</code>
getopt-style short option names. Can be any single character (unicode included) except a digit or hypen.

```js
[
  { name: "hot", alias: "h", type: Boolean },
  { name: "discount", alias: "d", type: Boolean },
  { name: "courses", alias: "c" , type: Number }
]
```

| #   | Command line | .parse() output |
| --- | ------------ | ------------ |
| 1   | `-hcd` | `{ hot: true, courses: null, discount: true }` |
| 2   | `-hdc 3` | `{ hot: true, discount: true, courses: 3 }` |

**Kind**: instance property of <code>[OptionDefinition](#exp_module_definition--OptionDefinition)</code>  
<a name="module_definition--OptionDefinition.OptionDefinition+multiple"></a>

### option.multiple : <code>boolean</code>
Set this flag if the option takes a list of values. You will receive an array of values, each passed through the `type` function (if specified).

```js
[
  { name: "files", type: String, multiple: true }
]
```

| #   | Command line | .parse() output |
| --- | ------------ | ------------ |
| 1   | `--files one.js two.js` | `{ files: [ 'one.js', 'two.js' ] }` |
| 2   | `--files one.js --files two.js` | `{ files: [ 'one.js', 'two.js' ] }` |
| 3   | `--files *` | `{ files: [ 'one.js', 'two.js' ] }` |

**Kind**: instance property of <code>[OptionDefinition](#exp_module_definition--OptionDefinition)</code>  
<a name="module_definition--OptionDefinition.OptionDefinition+defaultOption"></a>

### option.defaultOption : <code>boolean</code>
Any unclaimed command-line args will be set on this option. This flag is typically set on the most commonly-used option to make for more concise usage (i.e. `$ myapp *.js` instead of `$ myapp --files *.js`).

```js
[
  { name: "files", type: String, multiple: true, defaultOption: true }
]
```

| #   | Command line | .parse() output |
| --- | ------------ | ------------ |
| 1   | `--files one.js two.js` | `{ files: [ 'one.js', 'two.js' ] }` |
| 2   | `one.js two.js` | `{ files: [ 'one.js', 'two.js' ] }` |
| 3   | `*` | `{ files: [ 'one.js', 'two.js' ] }` |

**Kind**: instance property of <code>[OptionDefinition](#exp_module_definition--OptionDefinition)</code>  
<a name="module_definition--OptionDefinition.OptionDefinition+defaultValue"></a>

### option.defaultValue : <code>\*</code>
An initial value for the option.

```js
[
  { name: "files", type: String, multiple: true, defaultValue: [ "one.js" ] },
  { name: "max", type: Number, defaultValue: 3 }
]
```

| #   | Command line | .parse() output |
| --- | ------------ | ------------ |
| 1   |  | `{ files: [ 'one.js' ], max: 3 }` |
| 2   | `--files two.js` | `{ files: [ 'two.js' ], max: 3 }` |
| 3   | `--max 4` | `{ files: [ 'one.js' ], max: 4 }` |

**Kind**: instance property of <code>[OptionDefinition](#exp_module_definition--OptionDefinition)</code>  
<a name="module_definition--OptionDefinition.OptionDefinition+group"></a>

### option.group : <code>string</code> \| <code>Array.&lt;string&gt;</code>
When your app has a large amount of options it makes sense to organise them in groups.

There are two automatic groups: `_all` (contains all options) and `_none` (contains options without a `group` specified in their definition).

```js
[
  { name: "verbose", group: "standard" },
  { name: "help", group: [ "standard", "main" ] },
  { name: "compress", group: [ "server", "main" ] },
  { name: "static", group: "server" },
  { name: "debug" }
]
```

<table>
 <tr>
   <th>#</th><th>Command Line</th><th>.parse() output</th>
 </tr>
 <tr>
   <td>1</td><td><code>--verbose</code></td><td><pre><code>
{
 _all: { verbose: true },
 standard: { verbose: true }
}
</code></pre></td>
 </tr>
 <tr>
   <td>2</td><td><code>--debug</code></td><td><pre><code>
{
 _all: { debug: true },
 _none: { debug: true }
}
</code></pre></td>
 </tr>
 <tr>
   <td>3</td><td><code>--verbose --debug --compress</code></td><td><pre><code>
{
 _all: {
   verbose: true,
   debug: true,
   compress: true
 },
 standard: { verbose: true },
 server: { compress: true },
 main: { compress: true },
 _none: { debug: true }
}
</code></pre></td>
 </tr>
 <tr>
   <td>4</td><td><code>--compress</code></td><td><pre><code>
{
 _all: { compress: true },
 server: { compress: true },
 main: { compress: true }
}
</code></pre></td>
 </tr>
</table>

**Kind**: instance property of <code>[OptionDefinition](#exp_module_definition--OptionDefinition)</code>  


* * *

&copy; 2014-17 Lloyd Brookes \<75pound@gmail.com\>. Documented by [jsdoc-to-markdown](https://github.com/75lb/jsdoc-to-markdown).
