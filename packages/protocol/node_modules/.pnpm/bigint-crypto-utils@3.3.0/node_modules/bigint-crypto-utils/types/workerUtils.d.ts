export declare function _workerUrl(workerCode: string): string;
declare let _useWorkers: boolean;
export { _useWorkers };
export interface WorkerToMainMsg {
    _bcu: {
        isPrime: boolean;
        value: bigint;
        id: number;
    };
}
export interface MainToWorkerMsg {
    _bcu: {
        rnd: bigint;
        iterations: number;
        id: number;
    };
}
//# sourceMappingURL=workerUtils.d.ts.map