"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.DefenderApiResponseError = void 0;
const fp_1 = require("lodash/fp");
class DefenderApiResponseError extends Error {
    constructor(axiosError) {
        super(axiosError.message);
        this.name = 'DefenderApiResponseError';
        this.request = (0, fp_1.pick)(['path', 'method'], axiosError.request);
        this.response = (0, fp_1.pick)(['status', 'statusText', 'data'], axiosError.response);
    }
}
exports.DefenderApiResponseError = DefenderApiResponseError;
