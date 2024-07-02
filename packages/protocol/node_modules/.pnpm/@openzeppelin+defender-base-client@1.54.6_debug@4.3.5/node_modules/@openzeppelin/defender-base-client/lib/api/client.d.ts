/// <reference types="node" />
import { AxiosInstance } from 'axios';
import https from 'https';
import { AuthType } from './auth-v2';
export type ApiVersion = 'v1' | 'v2';
export type AuthConfig = {
    useCredentialsCaching: boolean;
    type: AuthType;
};
export declare abstract class BaseApiClient {
    private api;
    private version;
    private apiKey;
    private session;
    private sessionV2;
    private apiSecret;
    private httpsAgent?;
    private authConfig;
    protected abstract getPoolId(): string;
    protected abstract getPoolClientId(): string;
    protected abstract getApiUrl(v: ApiVersion, type?: AuthType): string;
    constructor(params: {
        apiKey: string;
        apiSecret: string;
        httpsAgent?: https.Agent;
        authConfig?: AuthConfig;
    });
    private getAccessToken;
    private getAccessTokenV2;
    private refreshSession;
    private refreshSessionV2;
    protected init(v?: ApiVersion): Promise<AxiosInstance>;
    protected refresh(v?: ApiVersion): Promise<AxiosInstance>;
    protected apiCall<T>(fn: (api: AxiosInstance) => Promise<T>, v?: ApiVersion): Promise<T>;
}
//# sourceMappingURL=client.d.ts.map