import { BaseApiClient, ApiVersion } from '@openzeppelin/defender-base-client';
import { Hex, Address, ExternalApiCreateProposalRequest as CreateProposalRequest } from './models/proposal';
import { SimulationRequest as SimulationTransaction, SimulationResponse } from './models/simulation';
import { Contract } from './models/contract';
import { ProposalListPaginatedResponse, ExternalApiProposalResponse as ProposalResponse } from './models/response';
import { Verification, VerificationRequest } from './models/verification';
type UpgradeParams = {
    title?: string;
    description?: string;
    proxyAdmin?: string;
    via?: Address;
    viaType?: CreateProposalRequest['viaType'];
    newImplementation: string;
    newImplementationAbi?: string;
    relayerId?: string;
};
type PauseParams = {
    title?: string;
    description?: string;
    via: Address;
    viaType: CreateProposalRequest['viaType'];
    relayerId?: string;
};
type AccessControlParams = {
    title?: string;
    description?: string;
    via: Address;
    viaType: CreateProposalRequest['viaType'];
    relayerId?: string;
};
export interface ProposalResponseWithUrl extends ProposalResponse {
    url: string;
    simulation?: SimulationResponse;
}
export declare class AdminClient extends BaseApiClient {
    protected getPoolId(): string;
    protected getPoolClientId(): string;
    protected getApiUrl(v?: ApiVersion): string;
    addContract(contract: Contract): Promise<Contract>;
    deleteContract(contractId: string): Promise<string>;
    listContracts(): Promise<Omit<Contract, 'abi'>[]>;
    createProposal(proposal: CreateProposalRequest & {
        simulate?: boolean;
        overrideSimulationOpts?: SimulationTransaction;
    }): Promise<ProposalResponseWithUrl>;
    listProposals(opts?: {
        limit?: number;
        next?: string;
        includeArchived?: boolean;
    }): Promise<ProposalResponseWithUrl[] | ProposalListPaginatedResponse>;
    getProposal(contractId: string, proposalId: string): Promise<ProposalResponseWithUrl>;
    archiveProposal(_: string, proposalId: string): Promise<ProposalResponseWithUrl>;
    unarchiveProposal(_: string, proposalId: string): Promise<ProposalResponseWithUrl>;
    getProposalSimulation(contractId: string, proposalId: string): Promise<SimulationResponse>;
    simulateProposal(contractId: string, proposalId: string, transaction: SimulationTransaction): Promise<SimulationResponse>;
    proposeUpgrade(params: UpgradeParams, contract: CreateProposalRequest['contract']): Promise<ProposalResponseWithUrl>;
    proposePause(params: PauseParams, contract: CreateProposalRequest['contract']): Promise<ProposalResponseWithUrl>;
    proposeUnpause(params: PauseParams, contract: CreateProposalRequest['contract']): Promise<ProposalResponseWithUrl>;
    proposeGrantRole(params: AccessControlParams, contract: CreateProposalRequest['contract'], role: Hex, account: Address): Promise<ProposalResponseWithUrl>;
    proposeRevokeRole(params: AccessControlParams, contract: CreateProposalRequest['contract'], role: Hex, account: Address): Promise<ProposalResponseWithUrl>;
    verifyDeployment(params: VerificationRequest): Promise<Verification>;
    getDeploymentVerification(params: Pick<VerificationRequest, 'contractAddress' | 'contractNetwork'>): Promise<Verification | undefined>;
    private proposePauseabilityAction;
    private proposeAccessControlAction;
}
export {};
//# sourceMappingURL=api.d.ts.map