typewise
=========

# typewiselite

compare sensible js data types

this is a fork of [typewise](https://github.com/deanlandolt/typewise)

## example

``` js

var compare = require('typewiselite')

ARRAY.sort(compare)

```

## ordering

* null
* boolean (false < true)
* numbers
* buffers
* strings
* object (!will throw)
* arrays
* undefined

ordering is the same as [bytewise](https://github.com/deanlandolt/bytewise)
except there is no support for {} objects.

## License

[MIT](http://deanlandolt.mit-license.org/)
