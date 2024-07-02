"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
const config_1 = require("hardhat/config");
const plugins_1 = require("hardhat/plugins");
const helpers_1 = require("./helpers");
require("./type-extensions");
const registerCustomInspection = (BigNumber) => {
    const inspectCustomSymbol = Symbol.for("nodejs.util.inspect.custom");
    BigNumber.prototype[inspectCustomSymbol] = function () {
        return `BigNumber { value: "${this.toString()}" }`;
    };
};
(0, config_1.extendEnvironment)((hre) => {
    hre.ethers = (0, plugins_1.lazyObject)(() => {
        const { createProviderProxy } = require("./provider-proxy");
        const { ethers } = require("ethers");
        registerCustomInspection(ethers.BigNumber);
        const providerProxy = createProviderProxy(hre.network.provider);
        return {
            ...ethers,
            provider: providerProxy,
            getSigner: (address) => (0, helpers_1.getSigner)(hre, address),
            getSigners: () => (0, helpers_1.getSigners)(hre),
            getImpersonatedSigner: (address) => (0, helpers_1.getImpersonatedSigner)(hre, address),
            // We cast to any here as we hit a limitation of Function#bind and
            // overloads. See: https://github.com/microsoft/TypeScript/issues/28582
            getContractFactory: helpers_1.getContractFactory.bind(null, hre),
            getContractFactoryFromArtifact: helpers_1.getContractFactoryFromArtifact.bind(null, hre),
            getContractAt: helpers_1.getContractAt.bind(null, hre),
            getContractAtFromArtifact: helpers_1.getContractAtFromArtifact.bind(null, hre),
            deployContract: helpers_1.deployContract.bind(null, hre),
        };
    });
});
//# sourceMappingURL=index.js.map