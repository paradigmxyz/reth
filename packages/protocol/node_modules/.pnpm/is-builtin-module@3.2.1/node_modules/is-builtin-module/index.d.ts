/**
Returns `true` if the given `moduleName` is a Node.js builtin module, `false` otherwise.

@param moduleName  - The name of the module.

@example
```
import isBuiltinModule = require('is-builtin-module');

isBuiltinModule('fs/promises');
//=> true

isBuiltinModule('node:fs');
//=> true

isBuiltinModule('unicorn');
//=> false
```
*/
declare function isBuiltinModule(moduleName: string): boolean;

export = isBuiltinModule;
