"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.sleep = exports.getTimestampInSeconds = void 0;
const getTimestampInSeconds = () => Math.floor(Date.now() / 1000);
exports.getTimestampInSeconds = getTimestampInSeconds;
const sleep = (millisecond) => new Promise((resolve) => setTimeout(resolve, millisecond));
exports.sleep = sleep;
