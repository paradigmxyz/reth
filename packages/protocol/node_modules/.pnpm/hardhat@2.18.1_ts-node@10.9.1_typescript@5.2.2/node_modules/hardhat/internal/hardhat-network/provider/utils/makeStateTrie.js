"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.makeStateTrie = void 0;
const ethereumjs_trie_1 = require("@nomicfoundation/ethereumjs-trie");
const ethereumjs_util_1 = require("@nomicfoundation/ethereumjs-util");
const makeAccount_1 = require("./makeAccount");
async function makeStateTrie(genesisAccounts) {
    const stateTrie = new ethereumjs_trie_1.Trie({ useKeyHashing: true });
    for (const acc of genesisAccounts) {
        const { address, account } = (0, makeAccount_1.makeAccount)(acc);
        await stateTrie.put(address.toBuffer(), account.serialize());
    }
    // Mimic precompiles activation
    for (let i = 1; i <= 8; i++) {
        await stateTrie.put((0, ethereumjs_util_1.setLengthLeft)((0, ethereumjs_util_1.intToBuffer)(i), 20), new ethereumjs_util_1.Account().serialize());
    }
    return stateTrie;
}
exports.makeStateTrie = makeStateTrie;
//# sourceMappingURL=makeStateTrie.js.map