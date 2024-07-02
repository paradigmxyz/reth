/// <reference types="node" />
import { AxiosError, AxiosInstance } from 'axios';
import https from 'https';
import { AuthType } from './auth-v2';
export type RetryConfig = {
    retries: number;
    retryDelay: (retryCount: number, error: AxiosError) => number;
    retryCondition?: (error: AxiosError) => boolean | Promise<boolean>;
};
export type AuthConfig = {
    useCredentialsCaching: boolean;
    type: AuthType;
};
type ApiFunction<TResponse> = (api: AxiosInstance) => Promise<TResponse>;
export declare abstract class BaseApiClient {
    private api;
    private apiKey;
    private session;
    private sessionV2;
    private apiSecret;
    private httpsAgent?;
    private retryConfig;
    private authConfig;
    protected abstract getPoolId(): string;
    protected abstract getPoolClientId(): string;
    protected abstract getApiUrl(type?: AuthType): string;
    constructor(params: {
        apiKey: string;
        apiSecret: string;
        httpsAgent?: https.Agent;
        retryConfig?: Partial<RetryConfig>;
        authConfig?: AuthConfig;
    });
    private getAccessToken;
    private getAccessTokenV2;
    private refreshSession;
    private refreshSessionV2;
    protected init(): Promise<AxiosInstance>;
    protected refresh(overrides?: {
        headers?: Record<string, string>;
    }): Promise<AxiosInstance>;
    private withRetry;
    protected apiCall<TResponse>(apiFunction: ApiFunction<TResponse>): Promise<TResponse>;
}
export declare const exponentialDelay: (retryNumber?: number, _error?: AxiosError | undefined, delayFactor?: number) => number;
export {};
//# sourceMappingURL=client.d.ts.map