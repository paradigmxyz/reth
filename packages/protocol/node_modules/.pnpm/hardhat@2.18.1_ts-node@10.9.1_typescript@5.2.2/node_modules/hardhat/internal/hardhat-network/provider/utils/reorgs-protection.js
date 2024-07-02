"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.FALLBACK_MAX_REORG = exports.getLargestPossibleReorg = void 0;
/**
 * This function returns a number that should be safe to consider as the
 * largest possible reorg in a network.
 *
 * If there's not such a number, or we aren't aware of it, this function
 * returns undefined.
 */
function getLargestPossibleReorg(networkId) {
    // mainnet
    if (networkId === 1) {
        return 5n;
    }
    // Kovan
    if (networkId === 42) {
        return 5n;
    }
    // Goerli
    if (networkId === 5) {
        return 5n;
    }
    // Rinkeby
    if (networkId === 4) {
        return 5n;
    }
    // Ropsten
    if (networkId === 3) {
        return 100n;
    }
    // xDai
    if (networkId === 100) {
        return 38n;
    }
}
exports.getLargestPossibleReorg = getLargestPossibleReorg;
exports.FALLBACK_MAX_REORG = 30n;
//# sourceMappingURL=reorgs-protection.js.map