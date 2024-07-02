export type AuthType = 'admin' | 'relay';
export type AuthCredentials = {
    apiKey: string;
    secretKey: string;
    type: AuthType;
};
export type RefreshCredentials = {
    apiKey: string;
    secretKey: string;
    refreshToken: string;
    type: AuthType;
};
export type AuthResponse = {
    accessToken: string;
    refreshToken: string;
};
export declare function authenticateV2(credentials: AuthCredentials, apiUrl: string): Promise<AuthResponse>;
export declare function refreshSessionV2(credentials: RefreshCredentials, apiUrl: string): Promise<AuthResponse>;
//# sourceMappingURL=auth-v2.d.ts.map