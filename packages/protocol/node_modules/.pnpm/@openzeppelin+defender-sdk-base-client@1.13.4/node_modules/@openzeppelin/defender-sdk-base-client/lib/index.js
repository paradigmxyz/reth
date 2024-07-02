"use strict";
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __exportStar = (this && this.__exportStar) || function(m, exports) {
    for (var p in m) if (p !== "default" && !Object.prototype.hasOwnProperty.call(exports, p)) __createBinding(exports, m, p);
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.DEFENDER_APP_URL = exports.VERSION = exports.BaseApiClient = exports.authenticate = exports.createAuthenticatedApi = exports.createApi = void 0;
var api_1 = require("./api/api");
Object.defineProperty(exports, "createApi", { enumerable: true, get: function () { return api_1.createApi; } });
Object.defineProperty(exports, "createAuthenticatedApi", { enumerable: true, get: function () { return api_1.createAuthenticatedApi; } });
var auth_1 = require("./api/auth");
Object.defineProperty(exports, "authenticate", { enumerable: true, get: function () { return auth_1.authenticate; } });
var client_1 = require("./api/client");
Object.defineProperty(exports, "BaseApiClient", { enumerable: true, get: function () { return client_1.BaseApiClient; } });
__exportStar(require("./utils/network"), exports);
// eslint-disable-next-line @typescript-eslint/no-var-requires
exports.VERSION = require('../package.json').version;
exports.DEFENDER_APP_URL = 'https://defender.openzeppelin.com';
