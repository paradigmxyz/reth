"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.ForkStateManager = void 0;
const ethereumjs_util_1 = require("@nomicfoundation/ethereumjs-util");
const immutable_1 = require("immutable");
const errors_1 = require("../../../core/errors");
const errors_2 = require("../../../core/providers/errors");
const keccak_1 = require("../../../util/keccak");
const makeAccount_1 = require("../utils/makeAccount");
const random_1 = require("../utils/random");
const AccountState_1 = require("./AccountState");
const encodeStorageKey = (address, position) => {
    return `${address.toString("hex")}${(0, ethereumjs_util_1.unpadBuffer)(position).toString("hex")}`;
};
const checkpointedError = (method) => new Error(`${method} called when checkpointed`);
const notCheckpointedError = (method) => new Error(`${method} called when not checkpointed`);
const notSupportedError = (method) => new Error(`${method} is not supported when forking from remote network`);
class ForkStateManager {
    constructor(_jsonRpcClient, _forkBlockNumber) {
        this._jsonRpcClient = _jsonRpcClient;
        this._forkBlockNumber = _forkBlockNumber;
        this._state = (0, immutable_1.Map)();
        this._initialStateRoot = (0, random_1.randomHash)();
        this._stateRoot = this._initialStateRoot;
        this._stateRootToState = new Map();
        this._originalStorageCache = new Map();
        this._stateCheckpoints = [];
        this._contextBlockNumber = this._forkBlockNumber;
        this._contextChanged = false;
        this._state = (0, immutable_1.Map)();
        this._stateRootToState.set(this._initialStateRoot, this._state);
    }
    async initializeGenesisAccounts(genesisAccounts) {
        const accounts = [];
        const noncesPromises = [];
        for (const ga of genesisAccounts) {
            const account = (0, makeAccount_1.makeAccount)(ga);
            accounts.push(account);
            const noncePromise = this._jsonRpcClient.getTransactionCount(account.address.toBuffer(), this._forkBlockNumber);
            noncesPromises.push(noncePromise);
        }
        const nonces = await Promise.all(noncesPromises);
        (0, errors_1.assertHardhatInvariant)(accounts.length === nonces.length, "Nonces and accounts should have the same length");
        for (const [index, { address, account }] of accounts.entries()) {
            const nonce = nonces[index];
            account.nonce = nonce;
            this._putAccount(address, account);
        }
        this._stateRootToState.set(this._initialStateRoot, this._state);
    }
    copy() {
        const fsm = new ForkStateManager(this._jsonRpcClient, this._forkBlockNumber);
        fsm._state = this._state;
        fsm._stateRoot = this._stateRoot;
        // because this map is append-only we don't need to copy it
        fsm._stateRootToState = this._stateRootToState;
        return fsm;
    }
    async getAccount(address) {
        const localAccount = this._state.get(address.toString());
        const localNonce = localAccount?.get("nonce");
        const localBalance = localAccount?.get("balance");
        const localCode = localAccount?.get("code");
        let nonce = localNonce !== undefined ? (0, ethereumjs_util_1.toBuffer)(localNonce) : undefined;
        let balance = localBalance !== undefined ? (0, ethereumjs_util_1.toBuffer)(localBalance) : undefined;
        let code = localCode !== undefined ? (0, ethereumjs_util_1.toBuffer)(localCode) : undefined;
        if (balance === undefined || nonce === undefined || code === undefined) {
            const accountData = await this._jsonRpcClient.getAccountData(address, this._contextBlockNumber);
            if (nonce === undefined) {
                nonce = accountData.transactionCount;
            }
            if (balance === undefined) {
                balance = accountData.balance;
            }
            if (code === undefined) {
                code = accountData.code;
            }
        }
        const codeHash = (0, keccak_1.keccak256)(code);
        // We ignore stateRoot since we found that it is not used anywhere of interest to us
        return ethereumjs_util_1.Account.fromAccountData({ nonce, balance, codeHash });
    }
    async putAccount(address, account) {
        this._putAccount(address, account);
    }
    touchAccount(_address) {
        // We don't do anything here. See cleanupTouchedAccounts for explanation
    }
    async putContractCode(address, value) {
        const hexAddress = address.toString();
        const account = (this._state.get(hexAddress) ?? (0, AccountState_1.makeAccountState)()).set("code", (0, ethereumjs_util_1.bufferToHex)(value));
        this._state = this._state.set(hexAddress, account);
    }
    async getContractCode(address) {
        const localCode = this._state.get(address.toString())?.get("code");
        if (localCode !== undefined) {
            return (0, ethereumjs_util_1.toBuffer)(localCode);
        }
        const accountData = await this._jsonRpcClient.getAccountData(address, this._contextBlockNumber);
        return accountData.code;
    }
    async getContractStorage(address, key) {
        if (key.length !== 32) {
            throw new Error("Storage key must be 32 bytes long");
        }
        const account = this._state.get(address.toString());
        const contractStorageCleared = account?.get("storageCleared") ?? false;
        const localValue = account?.get("storage").get((0, ethereumjs_util_1.bufferToHex)(key));
        if (localValue !== undefined) {
            return (0, ethereumjs_util_1.toBuffer)(localValue);
        }
        const slotCleared = localValue === null;
        if (contractStorageCleared || slotCleared) {
            return (0, ethereumjs_util_1.toBuffer)([]);
        }
        const remoteValue = await this._jsonRpcClient.getStorageAt(address, (0, ethereumjs_util_1.bufferToBigInt)(key), this._contextBlockNumber);
        return (0, ethereumjs_util_1.unpadBuffer)(remoteValue);
    }
    async putContractStorage(address, key, value) {
        if (key.length !== 32) {
            throw new Error("Storage key must be 32 bytes long");
        }
        if (value.length > 32) {
            throw new Error("Storage value cannot be longer than 32 bytes");
        }
        const unpaddedValue = (0, ethereumjs_util_1.unpadBuffer)(value);
        const hexAddress = address.toString();
        let account = this._state.get(hexAddress) ?? (0, AccountState_1.makeAccountState)();
        const currentStorage = account.get("storage");
        let newValue;
        if (unpaddedValue.length === 0) {
            // if the value is an empty array or only zeros, the storage is deleted
            newValue = null;
        }
        else {
            newValue = (0, ethereumjs_util_1.bufferToHex)(unpaddedValue);
        }
        const newStorage = currentStorage.set((0, ethereumjs_util_1.bufferToHex)(key), newValue);
        account = account.set("storage", newStorage);
        this._state = this._state.set(hexAddress, account);
    }
    async clearContractStorage(address) {
        const hexAddress = address.toString();
        let account = this._state.get(hexAddress) ?? (0, AccountState_1.makeAccountState)();
        account = account
            .set("storageCleared", true)
            .set("storage", (0, immutable_1.Map)());
        this._state = this._state.set(hexAddress, account);
    }
    async checkpoint() {
        const stateRoot = await this.getStateRoot();
        this._stateCheckpoints.push((0, ethereumjs_util_1.bufferToHex)(stateRoot));
    }
    async commit() {
        if (this._stateCheckpoints.length === 0) {
            throw notCheckpointedError("commit");
        }
        this._stateCheckpoints.pop();
    }
    async revert() {
        const checkpointedRoot = this._stateCheckpoints.pop();
        if (checkpointedRoot === undefined) {
            throw notCheckpointedError("revert");
        }
        await this.setStateRoot((0, ethereumjs_util_1.toBuffer)(checkpointedRoot));
    }
    async getStateRoot() {
        if (this._stateRootToState.get(this._stateRoot) !== this._state) {
            this._stateRoot = (0, random_1.randomHash)();
            this._stateRootToState.set(this._stateRoot, this._state);
        }
        return (0, ethereumjs_util_1.toBuffer)(this._stateRoot);
    }
    async setStateRoot(stateRoot) {
        this._setStateRoot(stateRoot);
    }
    async dumpStorage(_address) {
        throw notSupportedError("dumpStorage");
    }
    async hasGenesisState() {
        throw notSupportedError("hasGenesisState");
    }
    async generateCanonicalGenesis() {
        throw notSupportedError("generateCanonicalGenesis");
    }
    async generateGenesis(_initState) {
        throw notSupportedError("generateGenesis");
    }
    async accountIsEmpty(address) {
        const account = await this.getAccount(address);
        // From https://eips.ethereum.org/EIPS/eip-161
        // An account is considered empty when it has no code and zero nonce and zero balance.
        return (account.nonce === 0n &&
            account.balance === 0n &&
            account.codeHash.equals(ethereumjs_util_1.KECCAK256_NULL));
    }
    async cleanupTouchedAccounts() {
        // We do not do anything here, because cleaning accounts only affects the
        // stateRoot. Since the stateRoot is fake anyway there is no need to
        // perform this operation.
    }
    setBlockContext(stateRoot, blockNumber, irregularState) {
        if (this._stateCheckpoints.length !== 0) {
            throw checkpointedError("setBlockContext");
        }
        if (irregularState !== undefined) {
            this._setStateRoot(irregularState);
            return;
        }
        if (blockNumber === this._forkBlockNumber) {
            this._setStateRoot((0, ethereumjs_util_1.toBuffer)(this._initialStateRoot));
            return;
        }
        if (blockNumber > this._forkBlockNumber) {
            this._setStateRoot(stateRoot);
            return;
        }
        this._contextChanged = true;
        this._state = (0, immutable_1.Map)();
        this._stateRoot = (0, ethereumjs_util_1.bufferToHex)(stateRoot);
        this._stateRootToState.set(this._stateRoot, this._state);
        this._contextBlockNumber = blockNumber;
        // Note that we don't need to clear the original storage cache here
        // because the VM does it before executing a message anyway.
    }
    restoreForkBlockContext(stateRoot) {
        if (this._stateCheckpoints.length !== 0) {
            throw checkpointedError("restoreForkBlockContext");
        }
        this._setStateRoot(stateRoot);
        if (this._contextChanged) {
            this._contextChanged = false;
            this._contextBlockNumber = this._forkBlockNumber;
        }
    }
    accountExists(_address) {
        throw new errors_2.InternalError("Hardhat Network can't fork from networks running a hardfork older than Spurious Dragon");
    }
    async deleteAccount(address) {
        // we set an empty account instead of deleting it to avoid
        // re-fetching the state from the remote node.
        // This is only valid post spurious dragon, but we don't support older hardforks when forking.
        const emptyAccount = (0, AccountState_1.makeEmptyAccountState)();
        this._state = this._state.set(address.toString(), emptyAccount);
    }
    clearOriginalStorageCache() {
        this._originalStorageCache = new Map();
    }
    async getOriginalContractStorage(address, key) {
        const storageKey = encodeStorageKey(address.toBuffer(), key);
        const cachedValue = this._originalStorageCache.get(storageKey);
        if (cachedValue !== undefined) {
            return cachedValue;
        }
        const value = await this.getContractStorage(address, key);
        this._originalStorageCache.set(storageKey, value);
        return value;
    }
    _putAccount(address, account) {
        // Because the vm only ever modifies the nonce, balance and codeHash using this
        // method we ignore the stateRoot property
        const hexAddress = address.toString();
        let localAccount = this._state.get(hexAddress) ?? (0, AccountState_1.makeAccountState)();
        localAccount = localAccount
            .set("nonce", (0, ethereumjs_util_1.bigIntToHex)(account.nonce))
            .set("balance", (0, ethereumjs_util_1.bigIntToHex)(account.balance));
        // Code is set to empty string here to prevent unnecessary
        // JsonRpcClient.getCode calls in getAccount method
        if (account.codeHash.equals(ethereumjs_util_1.KECCAK256_NULL)) {
            localAccount = localAccount.set("code", "0x");
        }
        this._state = this._state.set(hexAddress, localAccount);
    }
    _setStateRoot(stateRoot) {
        const newRoot = (0, ethereumjs_util_1.bufferToHex)(stateRoot);
        const state = this._stateRootToState.get(newRoot);
        if (state === undefined) {
            throw new Error("Unknown state root");
        }
        this._stateRoot = newRoot;
        this._state = state;
    }
    async hasStateRoot(root) {
        return this._state.has((0, ethereumjs_util_1.bufferToHex)(root));
    }
    async flush() {
        // not implemented
    }
    async modifyAccountFields(address, accountFields) {
        // copied from BaseStateManager
        const account = await this.getAccount(address);
        account.nonce = accountFields.nonce ?? account.nonce;
        account.balance = accountFields.balance ?? account.balance;
        account.storageRoot = accountFields.storageRoot ?? account.storageRoot;
        account.codeHash = accountFields.codeHash ?? account.codeHash;
        await this.putAccount(address, account);
    }
}
exports.ForkStateManager = ForkStateManager;
//# sourceMappingURL=ForkStateManager.js.map