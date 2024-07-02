"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
const wasm_1 = require("./wasm");
/**
 * Class based SHA256
 */
class SHA256 {
    constructor() {
        this.ctx = wasm_1.newInstance();
        this.wasmInputValue = this.ctx.input.value;
        this.wasmOutputValue = this.ctx.output.value;
        this.uint8InputArray = new Uint8Array(this.ctx.memory.buffer, this.wasmInputValue, this.ctx.INPUT_LENGTH);
        this.uint8OutputArray = new Uint8Array(this.ctx.memory.buffer, this.wasmOutputValue, 32);
    }
    init() {
        this.ctx.init();
        return this;
    }
    update(data) {
        const INPUT_LENGTH = this.ctx.INPUT_LENGTH;
        if (data.length > INPUT_LENGTH) {
            for (let i = 0; i < data.length; i += INPUT_LENGTH) {
                const sliced = data.slice(i, i + INPUT_LENGTH);
                this.uint8InputArray.set(sliced);
                this.ctx.update(this.wasmInputValue, sliced.length);
            }
        }
        else {
            this.uint8InputArray.set(data);
            this.ctx.update(this.wasmInputValue, data.length);
        }
        return this;
    }
    final() {
        this.ctx.final(this.wasmOutputValue);
        const output = new Uint8Array(32);
        output.set(this.uint8OutputArray);
        return output;
    }
}
exports.default = SHA256;
//# sourceMappingURL=sha256.js.map