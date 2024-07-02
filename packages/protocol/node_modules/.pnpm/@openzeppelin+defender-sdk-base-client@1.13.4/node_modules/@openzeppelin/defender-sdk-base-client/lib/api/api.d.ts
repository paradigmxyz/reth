/// <reference types="node" />
import { AxiosError, AxiosInstance } from 'axios';
import https from 'https';
export declare function rejectWithDefenderApiError(axiosError: AxiosError): Promise<never>;
export declare function createApi(apiUrl: string, key?: string, token?: string, httpsAgent?: https.Agent, headers?: Record<string, string>): AxiosInstance;
export declare function createAuthenticatedApi(username: string, accessToken: string, apiUrl: string, httpsAgent?: https.Agent, headers?: Record<string, string>): AxiosInstance;
export declare function createUnauthorizedApi(apiUrl: string, httpsAgent?: https.Agent, headers?: Record<string, string>): AxiosInstance;
//# sourceMappingURL=api.d.ts.map