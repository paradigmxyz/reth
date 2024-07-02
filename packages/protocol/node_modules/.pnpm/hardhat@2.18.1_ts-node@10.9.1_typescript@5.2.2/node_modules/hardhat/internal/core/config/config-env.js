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
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || function (mod) {
    if (mod && mod.__esModule) return mod;
    var result = {};
    if (mod != null) for (var k in mod) if (k !== "default" && Object.prototype.hasOwnProperty.call(mod, k)) __createBinding(result, mod, k);
    __setModuleDefault(result, mod);
    return result;
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.experimentalAddHardhatNetworkMessageTraceHook = exports.extendProvider = exports.extendConfig = exports.extendEnvironment = exports.types = exports.scope = exports.internalTask = exports.subtask = exports.task = void 0;
const context_1 = require("../../context");
const argumentTypes = __importStar(require("../params/argumentTypes"));
function task(name, descriptionOrAction, action) {
    const ctx = context_1.HardhatContext.getHardhatContext();
    const dsl = ctx.tasksDSL;
    if (descriptionOrAction === undefined) {
        return dsl.task(name);
    }
    if (typeof descriptionOrAction !== "string") {
        return dsl.task(name, descriptionOrAction);
    }
    return dsl.task(name, descriptionOrAction, action);
}
exports.task = task;
function subtask(name, descriptionOrAction, action) {
    const ctx = context_1.HardhatContext.getHardhatContext();
    const dsl = ctx.tasksDSL;
    if (descriptionOrAction === undefined) {
        return dsl.subtask(name);
    }
    if (typeof descriptionOrAction !== "string") {
        return dsl.subtask(name, descriptionOrAction);
    }
    return dsl.subtask(name, descriptionOrAction, action);
}
exports.subtask = subtask;
// Backwards compatibility alias
exports.internalTask = subtask;
function scope(name, description) {
    const ctx = context_1.HardhatContext.getHardhatContext();
    const dsl = ctx.tasksDSL;
    return dsl.scope(name, description);
}
exports.scope = scope;
exports.types = argumentTypes;
/**
 * Register an environment extender what will be run after the
 * Hardhat Runtime Environment is initialized.
 *
 * @param extender A function that receives the Hardhat Runtime
 * Environment.
 */
function extendEnvironment(extender) {
    const ctx = context_1.HardhatContext.getHardhatContext();
    ctx.environmentExtenders.push(extender);
}
exports.extendEnvironment = extendEnvironment;
/**
 * Register a config extender what will be run after the
 * Hardhat Runtime Environment is initialized.
 *
 * @param extender A function that receives the resolved config
 * to be modified and the config provided by the user
 */
function extendConfig(extender) {
    const ctx = context_1.HardhatContext.getHardhatContext();
    ctx.configExtenders.push(extender);
}
exports.extendConfig = extendConfig;
/**
 * Register a provider extender what will be run after the
 * Hardhat Runtime Environment is initialized.
 *
 * @param extender A function that receives the current provider
 * and returns a new one.
 */
function extendProvider(extender) {
    const ctx = context_1.HardhatContext.getHardhatContext();
    ctx.providerExtenders.push(extender);
}
exports.extendProvider = extendProvider;
// NOTE: This is experimental and will be removed. Please contact our team
// if you are planning to use it.
function experimentalAddHardhatNetworkMessageTraceHook(hook) {
    const ctx = context_1.HardhatContext.getHardhatContext();
    ctx.experimentalHardhatNetworkMessageTraceHooks.push(hook);
}
exports.experimentalAddHardhatNetworkMessageTraceHook = experimentalAddHardhatNetworkMessageTraceHook;
//# sourceMappingURL=config-env.js.map