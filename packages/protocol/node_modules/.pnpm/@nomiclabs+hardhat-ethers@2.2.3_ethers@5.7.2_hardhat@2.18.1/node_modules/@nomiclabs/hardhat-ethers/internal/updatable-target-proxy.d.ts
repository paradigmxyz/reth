/**
 * Returns a read-only proxy that just forwards everything to a target,
 * and a function that can be used to change that underlying target
 */
export declare function createUpdatableTargetProxy<T extends object>(initialTarget: T): {
    proxy: T;
    setTarget: (target: T) => void;
};
//# sourceMappingURL=updatable-target-proxy.d.ts.map