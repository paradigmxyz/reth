"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.BaseActionClient = void 0;
const lambda_1 = require("../utils/lambda");
const rate_limit_1 = require("../utils/rate-limit");
const time_1 = require("../utils/time");
function cleanError(payload) {
    if (!payload) {
        return 'Error occurred, but error payload was not defined';
    }
    const error = (0, lambda_1.isV3ResponsePayload)(payload) ? payload.transformToString() : payload;
    try {
        const errMsg = JSON.parse(error.toString()).errorMessage;
        if (errMsg) {
            return errMsg;
        }
    }
    catch (e) { }
    return error;
}
class BaseActionClient {
    constructor(credentials, arn) {
        this.arn = arn;
        this.invocationRateLimit = rate_limit_1.rateLimitModule.createCounterFor(arn, 300);
        this.lambda = (0, lambda_1.getLambdaFromCredentials)(credentials);
    }
    async invoke(FunctionName, Payload) {
        if ((0, lambda_1.isLambdaV3)(this.lambda)) {
            return this.lambda.invoke({
                FunctionName,
                Payload,
                InvocationType: 'RequestResponse',
            });
        }
        else {
            return this.lambda
                .invoke({
                FunctionName,
                Payload,
                InvocationType: 'RequestResponse',
            })
                .promise();
        }
    }
    // eslint-disable-next-line @typescript-eslint/ban-types
    async execute(request) {
        const invocationTimeStamp = (0, time_1.getTimestampInSeconds)();
        this.invocationRateLimit.checkRateFor(invocationTimeStamp);
        this.invocationRateLimit.incrementRateFor(invocationTimeStamp);
        const invocationRequestResult = await this.invoke(this.arn, JSON.stringify(request));
        if (invocationRequestResult.FunctionError) {
            throw new Error(`Error while attempting request: ${cleanError(invocationRequestResult.Payload)}`);
        }
        return JSON.parse((0, lambda_1.isLambdaV3)(this.lambda)
            ? invocationRequestResult.Payload.transformToString()
            : invocationRequestResult.Payload);
    }
}
exports.BaseActionClient = BaseActionClient;
