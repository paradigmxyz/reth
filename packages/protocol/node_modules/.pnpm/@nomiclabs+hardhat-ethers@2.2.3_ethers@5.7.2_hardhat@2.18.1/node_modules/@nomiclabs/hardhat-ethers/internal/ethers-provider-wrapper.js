"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.EthersProviderWrapper = void 0;
const ethers_1 = require("ethers");
class EthersProviderWrapper extends ethers_1.ethers.providers.JsonRpcProvider {
    constructor(hardhatProvider) {
        super();
        this._hardhatProvider = hardhatProvider;
    }
    async send(method, params) {
        const result = await this._hardhatProvider.send(method, params);
        // We replicate ethers' behavior.
        this.emit("debug", {
            action: "send",
            request: {
                id: 42,
                jsonrpc: "2.0",
                method,
                params,
            },
            response: result,
            provider: this,
        });
        return result;
    }
    toJSON() {
        return "<WrappedHardhatProvider>";
    }
}
exports.EthersProviderWrapper = EthersProviderWrapper;
//# sourceMappingURL=ethers-provider-wrapper.js.map