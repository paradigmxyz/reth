"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.SignerWithAddress = void 0;
const ethers_1 = require("ethers");
class SignerWithAddress extends ethers_1.ethers.Signer {
    constructor(address, _signer) {
        super();
        this.address = address;
        this._signer = _signer;
        this.provider = _signer.provider;
    }
    static async create(signer) {
        return new SignerWithAddress(await signer.getAddress(), signer);
    }
    async getAddress() {
        return this.address;
    }
    signMessage(message) {
        return this._signer.signMessage(message);
    }
    signTransaction(transaction) {
        return this._signer.signTransaction(transaction);
    }
    sendTransaction(transaction) {
        return this._signer.sendTransaction(transaction);
    }
    connect(provider) {
        return new SignerWithAddress(this.address, this._signer.connect(provider));
    }
    _signTypedData(...params) {
        return this._signer._signTypedData(...params);
    }
    toJSON() {
        return `<SignerWithAddress ${this.address}>`;
    }
}
exports.SignerWithAddress = SignerWithAddress;
//# sourceMappingURL=signers.js.map