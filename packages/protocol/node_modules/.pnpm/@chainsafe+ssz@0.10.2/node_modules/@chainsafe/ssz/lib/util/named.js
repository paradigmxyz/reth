"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.namedClass = void 0;
function namedClass(superClass, className) {
    return new Function("superClass", `return class ${className} extends superClass {}`)(superClass);
}
exports.namedClass = namedClass;
//# sourceMappingURL=named.js.map