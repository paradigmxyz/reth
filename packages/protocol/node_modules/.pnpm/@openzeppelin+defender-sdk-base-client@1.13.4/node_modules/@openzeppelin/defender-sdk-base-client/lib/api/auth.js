"use strict";
// Adapted from https://gist.githubusercontent.com/efimk-lu/b48fa118bd29a35fc1767fe749fa3372/raw/0662fee3eb5c65172fdf85c4bdfcb96eabce5e21/authentication-example.js
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.refreshSession = exports.authenticate = void 0;
const amazon_cognito_identity_js_1 = require("amazon-cognito-identity-js");
const async_retry_1 = __importDefault(require("async-retry"));
async function authenticate(authenticationData, poolData) {
    const authenticationDetails = new amazon_cognito_identity_js_1.AuthenticationDetails(authenticationData);
    const userPool = new amazon_cognito_identity_js_1.CognitoUserPool(poolData);
    const userData = { Username: authenticationData.Username, Pool: userPool };
    const cognitoUser = new amazon_cognito_identity_js_1.CognitoUser(userData);
    try {
        return (0, async_retry_1.default)(() => doAuthenticate(cognitoUser, authenticationDetails), { retries: 3 });
    }
    catch (err) {
        const errorMessage = err.message || err;
        throw new Error(`Failed to get a token for the API key ${authenticationData.Username}: ${errorMessage}`);
    }
}
exports.authenticate = authenticate;
function doAuthenticate(cognitoUser, authenticationDetails) {
    return new Promise((resolve, reject) => {
        cognitoUser.authenticateUser(authenticationDetails, {
            onSuccess: function (session) {
                resolve(session);
            },
            onFailure: function (err) {
                reject(err);
            },
        });
    });
}
async function refreshSession(authenticationData, poolData, session) {
    const userPool = new amazon_cognito_identity_js_1.CognitoUserPool(poolData);
    const userData = { Username: authenticationData.Username, Pool: userPool };
    const cognitoUser = new amazon_cognito_identity_js_1.CognitoUser(userData);
    try {
        return (0, async_retry_1.default)(() => doRefreshSession(cognitoUser, session), { retries: 3 });
    }
    catch (err) {
        const errorMessage = err.message || err;
        throw new Error(`Failed to refresh token for the API key ${authenticationData.Username}: ${errorMessage}`);
    }
}
exports.refreshSession = refreshSession;
function doRefreshSession(cognitoUser, session) {
    return new Promise((resolve, reject) => {
        cognitoUser.refreshSession(session.getRefreshToken(), function (error, session) {
            if (error) {
                return reject(error);
            }
            resolve(session);
        });
    });
}
