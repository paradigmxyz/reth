"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.getLambdaFromCredentials = exports.isV3ResponsePayload = exports.isLambdaV3 = void 0;
const node_process_1 = require("node:process");
const NODE_MIN_VERSION_FOR_V3 = 18;
function isLambdaV3Compatible() {
    // example version: v14.17.0
    const majorVersion = node_process_1.version.slice(1).split('.')[0];
    if (!majorVersion)
        return false;
    return parseInt(majorVersion, 10) >= NODE_MIN_VERSION_FOR_V3;
}
function isLambdaV3(lambda) {
    return isLambdaV3Compatible();
}
exports.isLambdaV3 = isLambdaV3;
function isV3ResponsePayload(payload) {
    return payload.transformToString !== undefined;
}
exports.isV3ResponsePayload = isV3ResponsePayload;
function getLambdaFromCredentials(credentials) {
    const creds = credentials ? JSON.parse(credentials) : undefined;
    if (isLambdaV3Compatible()) {
        // eslint-disable-next-line @typescript-eslint/no-var-requires
        const { Lambda } = require('@aws-sdk/client-lambda');
        return new Lambda({
            credentials: {
                accessKeyId: creds.AccessKeyId,
                secretAccessKey: creds.SecretAccessKey,
                sessionToken: creds.SessionToken,
            },
        });
    }
    else {
        // eslint-disable-next-line @typescript-eslint/no-var-requires
        const Lambda = require('aws-sdk/clients/lambda');
        return new Lambda(creds
            ? {
                credentials: {
                    accessKeyId: creds.AccessKeyId,
                    secretAccessKey: creds.SecretAccessKey,
                    sessionToken: creds.SessionToken,
                },
            }
            : undefined);
    }
}
exports.getLambdaFromCredentials = getLambdaFromCredentials;
