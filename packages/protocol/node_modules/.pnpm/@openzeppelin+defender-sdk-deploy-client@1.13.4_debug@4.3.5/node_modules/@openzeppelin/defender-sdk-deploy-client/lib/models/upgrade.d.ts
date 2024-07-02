import { Network } from '@openzeppelin/defender-sdk-base-client';
import { Address } from '.';
export interface UpgradeContractRequest {
    proxyAddress: Address;
    senderAddress?: Address;
    proxyAdminAddress?: Address;
    newImplementationABI?: string;
    newImplementationAddress: Address;
    network: Network;
    approvalProcessId?: string;
}
export interface UpgradeContractResponse {
    proposalId: string;
    transaction?: ProposalTransaction;
    externalUrl?: string;
}
export interface ProposalTransaction {
    to: Address;
    from?: Address;
    value: BigUInt;
    data?: string;
    nonce: BigUInt;
    safe?: SafeTransactionDetails;
    executionTxHash?: Hex;
    confirmations?: ProposalTransactionConfirmation[];
    isSuccessful?: boolean;
    isExecuted?: boolean;
    isReverted?: boolean;
    fireblocksTransactionId?: string;
    relayerTransactionId?: string;
}
export interface SafeTransactionDetails {
    txGas: BigUInt;
    txHash: Hex;
    operationType?: 'call' | 'delegateCall';
}
export interface ProposalTransactionConfirmation {
    owner: Address;
    signature?: Hex;
}
export type BigUInt = string | number;
export type Hex = string;
//# sourceMappingURL=upgrade.d.ts.map