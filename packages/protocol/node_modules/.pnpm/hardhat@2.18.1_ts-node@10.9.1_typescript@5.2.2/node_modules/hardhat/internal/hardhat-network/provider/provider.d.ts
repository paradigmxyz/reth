/// <reference types="node" />
import type { Artifacts, BoundExperimentalHardhatNetworkMessageTraceHook, EIP1193Provider, HardhatNetworkChainsConfig, RequestArguments } from "../../../types";
import { EventEmitter } from "events";
import { ModulesLogger } from "./modules/logger";
import { ForkConfig, GenesisAccount, IntervalMiningConfig, MempoolOrder } from "./node-types";
export declare const DEFAULT_COINBASE = "0xc014ba5ec014ba5ec014ba5ec014ba5ec014ba5e";
interface HardhatNetworkProviderConfig {
    hardfork: string;
    chainId: number;
    networkId: number;
    blockGasLimit: number;
    minGasPrice: bigint;
    automine: boolean;
    intervalMining: IntervalMiningConfig;
    mempoolOrder: MempoolOrder;
    chains: HardhatNetworkChainsConfig;
    genesisAccounts: GenesisAccount[];
    allowUnlimitedContractSize: boolean;
    throwOnTransactionFailures: boolean;
    throwOnCallFailures: boolean;
    allowBlocksWithSameTimestamp: boolean;
    initialBaseFeePerGas?: number;
    initialDate?: Date;
    coinbase?: string;
    experimentalHardhatNetworkMessageTraceHooks?: BoundExperimentalHardhatNetworkMessageTraceHook[];
    forkConfig?: ForkConfig;
    forkCachePath?: string;
    enableTransientStorage: boolean;
}
export declare class HardhatNetworkProvider extends EventEmitter implements EIP1193Provider {
    private readonly _config;
    private readonly _logger;
    private readonly _artifacts?;
    private _node?;
    private _ethModule?;
    private _netModule?;
    private _web3Module?;
    private _evmModule?;
    private _hardhatModule?;
    private _debugModule?;
    private _personalModule?;
    private readonly _mutex;
    private _common?;
    constructor(_config: HardhatNetworkProviderConfig, _logger: ModulesLogger, _artifacts?: Artifacts | undefined);
    request(args: RequestArguments): Promise<unknown>;
    private _sendWithLogging;
    private _send;
    private _init;
    private _makeTracingConfig;
    private _makeMiningTimer;
    private _reset;
    private _forwardNodeEvents;
    private _stopForwardingNodeEvents;
    private _ethEventListener;
    private _emitLegacySubscriptionEvent;
    private _emitEip1193SubscriptionEvent;
}
export {};
//# sourceMappingURL=provider.d.ts.map