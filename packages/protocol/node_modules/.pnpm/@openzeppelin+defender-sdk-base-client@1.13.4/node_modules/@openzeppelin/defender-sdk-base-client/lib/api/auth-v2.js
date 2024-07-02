"use strict";
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.refreshSessionV2 = exports.authenticateV2 = void 0;
const async_retry_1 = __importDefault(require("async-retry"));
const api_1 = require("./api");
async function authenticateV2(credentials, apiUrl) {
    const api = (0, api_1.createUnauthorizedApi)(apiUrl);
    try {
        return await (0, async_retry_1.default)(() => api.post('/auth/login', credentials), { retries: 3 });
    }
    catch (err) {
        const errorMessage = err.response.statusText || err;
        throw new Error(`Failed to get a token for the API key ${credentials.apiKey}: ${errorMessage}`);
    }
}
exports.authenticateV2 = authenticateV2;
async function refreshSessionV2(credentials, apiUrl) {
    const api = (0, api_1.createUnauthorizedApi)(apiUrl);
    try {
        return await (0, async_retry_1.default)(() => api.post('/auth/refresh-token', credentials), { retries: 3 });
    }
    catch (err) {
        const errorMessage = err.response.statusText || err;
        throw new Error(`Failed to refresh token for the API key ${credentials.apiKey}: ${errorMessage}`);
    }
}
exports.refreshSessionV2 = refreshSessionV2;
