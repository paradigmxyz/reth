/// <reference types="node" />
export declare type TrieNode = BranchNode | ExtensionNode | LeafNode;
export declare type Nibbles = number[];
export declare type EmbeddedNode = Buffer | Buffer[];
export declare class BranchNode {
    _branches: (EmbeddedNode | null)[];
    _value: Buffer | null;
    constructor();
    static fromArray(arr: Buffer[]): BranchNode;
    get value(): Buffer | null;
    set value(v: Buffer | null);
    setBranch(i: number, v: EmbeddedNode | null): void;
    raw(): (EmbeddedNode | null)[];
    serialize(): Buffer;
    hash(): Buffer;
    getBranch(i: number): EmbeddedNode | null;
    getChildren(): [number, EmbeddedNode][];
}
export declare class ExtensionNode {
    _nibbles: Nibbles;
    _value: Buffer;
    constructor(nibbles: Nibbles, value: Buffer);
    static encodeKey(key: Nibbles): Nibbles;
    static decodeKey(key: Nibbles): Nibbles;
    get key(): Nibbles;
    set key(k: Nibbles);
    get keyLength(): number;
    get value(): Buffer;
    set value(v: Buffer);
    encodedKey(): Nibbles;
    raw(): [Buffer, Buffer];
    serialize(): Buffer;
    hash(): Buffer;
}
export declare class LeafNode {
    _nibbles: Nibbles;
    _value: Buffer;
    constructor(nibbles: Nibbles, value: Buffer);
    static encodeKey(key: Nibbles): Nibbles;
    static decodeKey(encodedKey: Nibbles): Nibbles;
    get key(): Nibbles;
    set key(k: Nibbles);
    get keyLength(): number;
    get value(): Buffer;
    set value(v: Buffer);
    encodedKey(): Nibbles;
    raw(): [Buffer, Buffer];
    serialize(): Buffer;
    hash(): Buffer;
}
export declare function decodeRawNode(raw: Buffer[]): TrieNode;
export declare function decodeNode(raw: Buffer): TrieNode;
export declare function isRawNode(n: any): boolean;
