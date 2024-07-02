"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.BaseApiClient = void 0;
const api_1 = require("./api");
const auth_1 = require("./auth");
const auth_v2_1 = require("./auth-v2");
class BaseApiClient {
    constructor(params) {
        if (!params.apiKey)
            throw new Error(`API key is required`);
        if (!params.apiSecret)
            throw new Error(`API secret is required`);
        this.apiKey = params.apiKey;
        this.apiSecret = params.apiSecret;
        this.httpsAgent = params.httpsAgent;
        this.authConfig = params.authConfig ?? { useCredentialsCaching: false, type: 'admin' };
    }
    async getAccessToken() {
        const userPass = { Username: this.apiKey, Password: this.apiSecret };
        const poolData = { UserPoolId: this.getPoolId(), ClientId: this.getPoolClientId() };
        this.session = await (0, auth_1.authenticate)(userPass, poolData);
        return this.session.getAccessToken().getJwtToken();
    }
    async getAccessTokenV2() {
        if (!this.authConfig.type)
            throw new Error('Auth type is required to authenticate in auth v2');
        const credentials = {
            apiKey: this.apiKey,
            secretKey: this.apiSecret,
            type: this.authConfig.type,
        };
        this.sessionV2 = await (0, auth_v2_1.authenticateV2)(credentials, this.getApiUrl('v1', 'admin'));
        return this.sessionV2.accessToken;
    }
    async refreshSession() {
        if (!this.session)
            return this.getAccessToken();
        const userPass = { Username: this.apiKey, Password: this.apiSecret };
        const poolData = { UserPoolId: this.getPoolId(), ClientId: this.getPoolClientId() };
        this.session = await (0, auth_1.refreshSession)(userPass, poolData, this.session);
        return this.session.getAccessToken().getJwtToken();
    }
    async refreshSessionV2() {
        if (!this.authConfig.type)
            throw new Error('Auth type is required to refresh session in auth v2');
        if (!this.sessionV2)
            return this.getAccessTokenV2();
        const credentials = {
            apiKey: this.apiKey,
            secretKey: this.apiSecret,
            refreshToken: this.sessionV2.refreshToken,
            type: this.authConfig.type,
        };
        this.sessionV2 = await (0, auth_v2_1.refreshSessionV2)(credentials, this.getApiUrl('v1', 'admin'));
        return this.sessionV2.accessToken;
    }
    async init(v = 'v1') {
        if (!this.api || this.version !== v) {
            const accessToken = this.authConfig.useCredentialsCaching
                ? await this.getAccessTokenV2()
                : await this.getAccessToken();
            this.api = (0, api_1.createAuthenticatedApi)(this.apiKey, accessToken, this.getApiUrl(v, this.authConfig.type ?? 'admin'), this.httpsAgent);
            this.version = v;
        }
        return this.api;
    }
    async refresh(v = 'v1') {
        if (!this.session && !this.sessionV2) {
            return this.init(v);
        }
        try {
            const accessToken = this.authConfig.useCredentialsCaching
                ? await this.refreshSessionV2()
                : await this.refreshSession();
            this.api = (0, api_1.createAuthenticatedApi)(this.apiKey, accessToken, this.getApiUrl(v, 'admin'), this.httpsAgent);
            return this.api;
        }
        catch (e) {
            return this.init(v);
        }
    }
    async apiCall(fn, v = 'v1') {
        const api = await this.init(v);
        try {
            return await fn(api);
        }
        catch (error) {
            // this means ID token has expired so we'll recreate session and try again
            if (error.response && error.response.status === 401 && error.response.statusText === 'Unauthorized') {
                this.api = undefined;
                const api = await this.refresh(v);
                return await fn(api);
            }
            throw error;
        }
    }
}
exports.BaseApiClient = BaseApiClient;
