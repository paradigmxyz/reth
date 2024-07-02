[![view on npm](https://img.shields.io/npm/v/array-back.svg)](https://www.npmjs.org/package/array-back)
[![npm module downloads](https://img.shields.io/npm/dt/array-back.svg)](https://www.npmjs.org/package/array-back)
[![Build Status](https://travis-ci.org/75lb/array-back.svg?branch=master)](https://travis-ci.org/75lb/array-back)
[![Coverage Status](https://coveralls.io/repos/github/75lb/array-back/badge.svg?branch=master)](https://coveralls.io/github/75lb/array-back?branch=master)
[![Dependency Status](https://david-dm.org/75lb/array-back.svg)](https://david-dm.org/75lb/array-back)
[![js-standard-style](https://img.shields.io/badge/code%20style-standard-brightgreen.svg)](https://github.com/feross/standard)

<a name="module_array-back"></a>

## array-back
**Example**  
```js
const arrayify = require('array-back')
```
<a name="exp_module_array-back--arrayify"></a>

### arrayify(input) ⇒ <code>Array</code> ⏏
Takes any input and guarantees an array back.

- converts array-like objects (e.g. `arguments`) to a real array
- converts `undefined` to an empty array
- converts any another other, singular value (including `null`) into an array containing that value
- ignores input which is already an array

**Kind**: Exported function  

| Param | Type | Description |
| --- | --- | --- |
| input | <code>\*</code> | the input value to convert to an array |

**Example**  
```js
> a.arrayify(undefined)
[]

> a.arrayify(null)
[ null ]

> a.arrayify(0)
[ 0 ]

> a.arrayify([ 1, 2 ])
[ 1, 2 ]

> function f(){ return a.arrayify(arguments); }
> f(1,2,3)
[ 1, 2, 3 ]
```

* * *

&copy; 2015-17 Lloyd Brookes \<75pound@gmail.com\>. Documented by [jsdoc-to-markdown](https://github.com/75lb/jsdoc-to-markdown).