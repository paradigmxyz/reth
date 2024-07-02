export type Address = string;
export type EventArgs = EventArgValues[];
/**
 * @minItems 3
 */
export type Log = [
    Address | string[] | string,
    Address | string[] | string,
    Address | string[] | string,
    ...(Address | string[] | string)[]
];
export type BigUInt = string | string | number;
export interface SimulationResponse {
    contractProposalId: string;
    createdAt: string;
    transaction: SimulationTransaction;
    meta: SimulationMetadata;
    states: ContractState[];
    events: LogEvent[];
    logs?: Log[];
    storage: StorageState[];
    traces: TransactionTrace[];
    transfers: TransactionTrace[];
}
export interface SimulationTransaction {
    to: string;
    from: string;
    data: string;
    value: string;
    nonce: number;
    gasPrice: number;
    gasLimit?: number;
}
export interface SimulationMetadata {
    network: string;
    blockNumber: number;
    forkedBlockNumber: number;
    simulatedBlockNumber: number;
    gasUsed: number;
    returnValue: string;
    returnString: string;
    reverted: boolean;
}
export interface ContractState {
    function: string;
    types: string[];
    states: {
        previous: string;
        current: string;
    };
}
export interface LogEvent {
    address: Address;
    name: string;
    signature: string;
    topics?: string[];
    args: EventArgs;
}
export interface EventArgValues {
    name?: string;
    type?: string;
    indexed?: boolean;
    value: string | Record<string, unknown>;
}
export interface StorageState {
    address: Address;
    slot: string;
    states: {
        previous?: string;
        current: string;
    };
}
export interface TransactionTrace {
    depth: number;
    type: string;
    gas: BigUInt;
    to: string;
    value: BigUInt;
    data: string;
}
export interface SimulationRequest {
    blockNumber?: BigUInt;
    transactionData: TransactionData;
}
export interface TransactionData {
    to: Address;
    from?: Address;
    value: BigUInt;
    data: string;
    [k: string]: unknown;
}
//# sourceMappingURL=simulation.d.ts.map