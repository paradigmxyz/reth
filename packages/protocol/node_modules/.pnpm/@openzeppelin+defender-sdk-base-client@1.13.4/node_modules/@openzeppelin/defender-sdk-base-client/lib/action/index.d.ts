export declare abstract class BaseActionClient {
    private arn;
    private lambda;
    private invocationRateLimit;
    constructor(credentials: string, arn: string);
    private invoke;
    protected execute<T>(request: object): Promise<T>;
}
//# sourceMappingURL=index.d.ts.map