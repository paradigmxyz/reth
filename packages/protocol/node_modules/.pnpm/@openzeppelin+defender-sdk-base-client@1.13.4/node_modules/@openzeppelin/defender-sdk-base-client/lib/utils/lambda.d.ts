/// <reference types="node" />
export type InvokeResponse = {
    FunctionError?: string;
    Payload: PayloadResponseV2 | PayloadResponseV3;
};
export type InvokeResponseV2 = {
    promise: () => Promise<InvokeResponse>;
};
export type PayloadResponseV3 = {
    transformToString: () => string;
};
export type PayloadResponseV2 = string | Buffer | Uint8Array | Blob;
export type LambdaV2 = {
    invoke: (params: {
        FunctionName: string;
        Payload: string;
        InvocationType: string;
    }) => InvokeResponseV2;
};
export type LambdaV3 = {
    invoke: (params: {
        FunctionName: string;
        Payload: string;
        InvocationType: string;
    }) => Promise<InvokeResponse>;
};
export type LambdaLike = LambdaV2 | LambdaV3;
export type LambdaCredentials = {
    AccessKeyId: string;
    SecretAccessKey: string;
    SessionToken: string;
};
export declare function isLambdaV3(lambda: LambdaLike): lambda is LambdaV3;
export declare function isV3ResponsePayload(payload: PayloadResponseV2 | PayloadResponseV3): payload is PayloadResponseV3;
export declare function getLambdaFromCredentials(credentials: string): LambdaLike;
//# sourceMappingURL=lambda.d.ts.map