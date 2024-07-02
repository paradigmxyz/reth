import { WasmContext } from "./wasm";
/**
 * Class based SHA256
 */
export default class SHA256 {
    ctx: WasmContext;
    private wasmInputValue;
    private wasmOutputValue;
    private uint8InputArray;
    private uint8OutputArray;
    constructor();
    init(): this;
    update(data: Uint8Array): this;
    final(): Uint8Array;
}
//# sourceMappingURL=sha256.d.ts.map