/// <reference types="node" />
import type EthereumjsUtilT from "@nomicfoundation/ethereumjs-util";
export declare class RandomBufferGenerator {
    private _nextValue;
    private constructor();
    static create(seed: string): RandomBufferGenerator;
    next(): Buffer;
    setNext(nextValue: Buffer): void;
    clone(): RandomBufferGenerator;
}
export declare const randomHash: () => string;
export declare const randomHashBuffer: () => Buffer;
export declare const randomAddress: () => EthereumjsUtilT.Address;
export declare const randomAddressString: () => string;
export declare const randomAddressBuffer: () => Buffer;
//# sourceMappingURL=random.d.ts.map