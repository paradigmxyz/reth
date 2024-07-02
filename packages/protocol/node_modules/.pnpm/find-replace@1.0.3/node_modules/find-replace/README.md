[![view on npm](http://img.shields.io/npm/v/find-replace.svg)](https://www.npmjs.org/package/find-replace)
[![npm module downloads](http://img.shields.io/npm/dt/find-replace.svg)](https://www.npmjs.org/package/find-replace)
[![Build Status](https://travis-ci.org/75lb/find-replace.svg?branch=master)](https://travis-ci.org/75lb/find-replace)
[![Dependency Status](https://david-dm.org/75lb/find-replace.svg)](https://david-dm.org/75lb/find-replace)
[![js-standard-style](https://img.shields.io/badge/code%20style-standard-brightgreen.svg)](https://github.com/feross/standard)

<a name="module_find-replace"></a>

## find-replace
Find and either replace or remove items from an array.

**Example**  
```js
> findReplace = require('find-replace')

> findReplace([ 1, 2, 3], 2, 'two')
[ 1, 'two', 3 ]

> findReplace([ 1, 2, 3], 2, [ 'two', 'zwei' ])
[ 1, [ 'two', 'zwei' ], 3 ]

> findReplace([ 1, 2, 3], 2, 'two', 'zwei')
[ 1, 'two', 'zwei', 3 ]

> findReplace([ 1, 2, 3], 2) // no replacement, so remove
[ 1, 3 ]
```
<a name="exp_module_find-replace--findReplace"></a>

### findReplace(array, valueTest, [...replaceWith]) ⇒ <code>array</code> ⏏
**Kind**: Exported function  

| Param | Type | Description |
| --- | --- | --- |
| array | <code>array</code> | the input array |
| valueTest | <code>valueTest</code> | a [test-value](https://github.com/75lb/test-value) query to match the value you're looking for |
| [...replaceWith] | <code>any</code> | If specified, found values will be replaced with these values, else  removed. |


* * *

&copy; 2015-17 Lloyd Brookes \<75pound@gmail.com\>. Documented by [jsdoc-to-markdown](https://github.com/jsdoc2md/jsdoc-to-markdown).
