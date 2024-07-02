"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.newInstance = void 0;
const wasmCode_1 = require("./wasmCode");
const _module = new WebAssembly.Module(wasmCode_1.wasmCode);
const importObj = {
    env: {
        // modified from https://github.com/AssemblyScript/assemblyscript/blob/v0.9.2/lib/loader/index.js#L70
        abort: function (msg, file, line, col) {
            throw Error(`abort: ${msg}:${file}:${line}:${col}`);
        },
    },
};
function newInstance() {
    return new WebAssembly.Instance(_module, importObj).exports;
}
exports.newInstance = newInstance;
//# sourceMappingURL=wasm.js.map