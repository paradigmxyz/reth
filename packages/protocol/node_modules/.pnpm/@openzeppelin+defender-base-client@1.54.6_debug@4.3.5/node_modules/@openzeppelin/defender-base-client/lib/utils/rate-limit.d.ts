export type RateLimitModule = {
    checkRateFor: (currentTimeStamp: number) => number;
    incrementRateFor: (currentTimeStamp: number) => number;
};
export declare const rateLimitModule: {
    createCounterFor: (toLimitIdentifier: string, rateLimit: number) => {
        getRateFor: (rateEntry: unknown) => number;
        checkRateFor: (rateEntry: unknown) => number;
        incrementRateFor: (rateEntry: unknown) => number;
    };
};
//# sourceMappingURL=rate-limit.d.ts.map