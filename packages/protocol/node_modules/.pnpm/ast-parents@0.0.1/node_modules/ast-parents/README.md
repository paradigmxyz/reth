# ast-parents [![Flattr this!](https://api.flattr.com/button/flattr-badge-large.png)](https://flattr.com/submit/auto?user_id=hughskennedy&url=http://github.com/hughsk/ast-parents&title=ast-parents&description=hughsk/ast-parents%20on%20GitHub&language=en_GB&tags=flattr,github,javascript&category=software)[![experimental](http://hughsk.github.io/stability-badges/dist/experimental.svg)](http://github.com/hughsk/stability-badges) #

Walks a JavaScript AST, such as one supplied via
[esprima](http://github.com/ariya/esprima), and adds a `parent`
property to each node.

Makes it much easier to navigate the AST, and the `parent` properties
added here are non-enumerable so you can still serialize the tree to JSON
without `JSON.stringify` throwing an error.

## Usage ##

[![ast-parents](https://nodei.co/npm/ast-parents.png?mini=true)](https://nodei.co/npm/ast-parents)

`require('ast-parents')(ast)`

Where `ast` is an AST object. For example:

``` javascript
var esprima = require('esprima')
var fs = require('fs')

var src = fs.readFileSync(__filename, 'utf8')
var ast = esprima.parse(src)

parents(ast)

ast.body[0].parent === ast.body
```

## License ##

MIT. See [LICENSE.md](http://github.com/hughsk/ast-parents/blob/master/LICENSE.md) for details.
