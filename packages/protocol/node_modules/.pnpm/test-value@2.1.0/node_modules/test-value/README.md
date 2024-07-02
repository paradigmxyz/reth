[![view on npm](http://img.shields.io/npm/v/test-value.svg)](https://www.npmjs.org/package/test-value)
[![npm module downloads](http://img.shields.io/npm/dt/test-value.svg)](https://www.npmjs.org/package/test-value)
[![Build Status](https://travis-ci.org/75lb/test-value.svg?branch=master)](https://travis-ci.org/75lb/test-value)
[![Dependency Status](https://david-dm.org/75lb/test-value.svg)](https://david-dm.org/75lb/test-value)
[![js-standard-style](https://img.shields.io/badge/code%20style-standard-brightgreen.svg)](https://github.com/feross/standard)

<a name="module_test-value"></a>

## test-value
**Example**  
```js
var testValue = require('test-value')
```

* [test-value](#module_test-value)
    * [testValue(value, test, [options])](#exp_module_test-value--testValue) ⇒ <code>boolean</code> ⏏
        * [.where(test)](#module_test-value--testValue.where) ⇒ <code>function</code>

<a name="exp_module_test-value--testValue"></a>

### testValue(value, test, [options]) ⇒ <code>boolean</code> ⏏
**Kind**: Exported function  

| Param | Type | Description |
| --- | --- | --- |
| value | <code>any</code> | a value to test |
| test | <code>any</code> | the test query |
| [options] | <code>object</code> |  |
| [options.strict] | <code>boolean</code> | Treat an object like a value not a query. |

<a name="module_test-value--testValue.where"></a>

#### testValue.where(test) ⇒ <code>function</code>
Returns a callback suitable for use by `Array` methods like `some`, `filter`, `find` etc.

**Kind**: static method of <code>[testValue](#exp_module_test-value--testValue)</code>  

| Param | Type | Description |
| --- | --- | --- |
| test | <code>any</code> | the test query |


* * *

&copy; 2015-16 Lloyd Brookes \<75pound@gmail.com\>. Documented by [jsdoc-to-markdown](https://github.com/jsdoc2md/jsdoc-to-markdown).
