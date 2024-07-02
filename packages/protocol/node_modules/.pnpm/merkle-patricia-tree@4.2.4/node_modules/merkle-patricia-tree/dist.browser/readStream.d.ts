import { Trie as BaseTrie } from './baseTrie';
declare const Readable: any;
export declare class TrieReadStream extends Readable {
    private trie;
    private _started;
    constructor(trie: BaseTrie);
    _read(): Promise<void>;
}
export {};
