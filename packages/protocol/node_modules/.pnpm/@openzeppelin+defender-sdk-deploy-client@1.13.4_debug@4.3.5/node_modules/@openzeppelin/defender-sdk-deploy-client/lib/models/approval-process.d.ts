import { Network } from '@openzeppelin/defender-sdk-base-client';
import { Address } from '.';
export type ComponentType = ('deploy' | 'upgrade')[];
export interface ApprovalProcessResponse {
    approvalProcessId: string;
    createdAt: string;
    name: string;
    component?: ComponentType;
    network?: Network;
    via?: Address;
    viaType?: 'EOA' | 'Contract' | 'Multisig' | 'Safe' | 'Gnosis Multisig' | 'Relayer' | 'Unknown' | 'Timelock Controller' | 'ERC20' | 'Governor' | 'Fireblocks';
    timelock?: Timelock;
    fireblocks?: FireblocksProposalParams;
    relayerId?: string;
    stackResourceId?: string;
}
export interface Timelock {
    address: Address;
    delay: string;
}
export interface FireblocksProposalParams {
    apiKeyId: string;
    vaultId: string;
    assetId: string;
}
//# sourceMappingURL=approval-process.d.ts.map