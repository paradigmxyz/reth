"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.rateLimitModule = void 0;
const createRateLimitModule = () => {
    const globalCounter = {};
    return {
        createCounterFor: (toLimitIdentifier, rateLimit) => {
            const hasAlreadyACounter = Boolean(globalCounter[toLimitIdentifier]);
            if (!hasAlreadyACounter)
                globalCounter[toLimitIdentifier] = new Map();
            const rateLimitForIdentifier = globalCounter[toLimitIdentifier];
            if (!rateLimitForIdentifier)
                throw new Error('Rate limit identifier not found');
            return {
                getRateFor: (rateEntry) => rateLimitForIdentifier.get(rateEntry) || 0,
                checkRateFor: (rateEntry) => {
                    const currentSecondNumberOfRequests = rateLimitForIdentifier.get(rateEntry) || 0;
                    if (currentSecondNumberOfRequests > rateLimit)
                        throw new Error('Rate limit exceeded');
                    return currentSecondNumberOfRequests;
                },
                incrementRateFor: (rateEntry) => {
                    const newRateCount = (rateLimitForIdentifier.get(rateEntry) || 0) + 1;
                    rateLimitForIdentifier.set(rateEntry, newRateCount);
                    return newRateCount;
                },
            };
        },
    };
};
exports.rateLimitModule = createRateLimitModule();
