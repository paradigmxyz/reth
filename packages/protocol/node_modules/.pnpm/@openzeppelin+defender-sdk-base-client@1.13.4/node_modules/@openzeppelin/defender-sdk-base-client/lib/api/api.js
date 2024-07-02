"use strict";
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.createUnauthorizedApi = exports.createAuthenticatedApi = exports.createApi = exports.rejectWithDefenderApiError = void 0;
const axios_1 = __importDefault(require("axios"));
const api_error_1 = require("./api-error");
function rejectWithDefenderApiError(axiosError) {
    return Promise.reject(new api_error_1.DefenderApiResponseError(axiosError));
}
exports.rejectWithDefenderApiError = rejectWithDefenderApiError;
function createApi(apiUrl, key, token, httpsAgent, headers) {
    const authHeaders = key && token
        ? {
            'X-Api-Key': key,
            'Authorization': `Bearer ${token}`,
        }
        : {};
    const instance = axios_1.default.create({
        baseURL: apiUrl,
        headers: {
            'Content-Type': 'application/json',
            ...authHeaders,
            ...headers,
        },
        httpsAgent,
    });
    instance.interceptors.response.use(({ data }) => data, rejectWithDefenderApiError);
    return instance;
}
exports.createApi = createApi;
function createAuthenticatedApi(username, accessToken, apiUrl, httpsAgent, headers) {
    return createApi(apiUrl, username, accessToken, httpsAgent, headers);
}
exports.createAuthenticatedApi = createAuthenticatedApi;
function createUnauthorizedApi(apiUrl, httpsAgent, headers) {
    return createApi(apiUrl, undefined, undefined, httpsAgent, headers);
}
exports.createUnauthorizedApi = createUnauthorizedApi;
