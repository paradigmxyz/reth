export interface WasmContext {
    readonly INPUT_LENGTH: number;
    memory: {
        buffer: ArrayBuffer;
    };
    input: {
        value: number;
    };
    output: {
        value: number;
    };
    init(): void;
    update(dataPtr: number, dataLength: number): void;
    final(outPtr: number): void;
    digest(length: number): void;
    digest64(inPtr: number, outPtr: number): void;
}
export declare function newInstance(): WasmContext;
//# sourceMappingURL=wasm.d.ts.map