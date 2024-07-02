/// <reference types="node" />
import { ConfigExtender, EnvironmentExtender, ExperimentalHardhatNetworkMessageTraceHook, HardhatRuntimeEnvironment, ProviderExtender } from "../types";
import { TasksDSL } from "./core/tasks/dsl";
export type GlobalWithHardhatContext = typeof global & {
    __hardhatContext: HardhatContext;
};
export declare class HardhatContext {
    static isCreated(): boolean;
    static createHardhatContext(): HardhatContext;
    static getHardhatContext(): HardhatContext;
    static deleteHardhatContext(): void;
    readonly tasksDSL: TasksDSL;
    readonly environmentExtenders: EnvironmentExtender[];
    environment?: HardhatRuntimeEnvironment;
    readonly providerExtenders: ProviderExtender[];
    readonly configExtenders: ConfigExtender[];
    readonly experimentalHardhatNetworkMessageTraceHooks: ExperimentalHardhatNetworkMessageTraceHook[];
    private _filesLoadedBeforeConfig?;
    private _filesLoadedAfterConfig?;
    setHardhatRuntimeEnvironment(env: HardhatRuntimeEnvironment): void;
    getHardhatRuntimeEnvironment(): HardhatRuntimeEnvironment;
    setConfigLoadingAsStarted(): void;
    setConfigLoadingAsFinished(): void;
    getFilesLoadedDuringConfig(): string[];
}
//# sourceMappingURL=context.d.ts.map