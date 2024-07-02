import { BaseApiClient, Network } from '@openzeppelin/defender-sdk-base-client';
import { ApprovalProcessResponse, BlockExplorerApiKeyResponse, CreateBlockExplorerApiKeyRequest, DeployContractRequest, DeploymentResponse, RemoveBlockExplorerApiKeyResponse, UpdateBlockExplorerApiKeyRequest, UpgradeContractRequest, UpgradeContractResponse } from '../models';
import { Verification, VerificationRequest } from '../models/verification';
export declare class DeployClient extends BaseApiClient {
    protected getPoolId(): string;
    protected getPoolClientId(): string;
    protected getApiUrl(): string;
    deployContract(params: DeployContractRequest): Promise<DeploymentResponse>;
    getDeployedContract(id: string): Promise<DeploymentResponse>;
    listDeployments(): Promise<DeploymentResponse[]>;
    getDeployApprovalProcess(network: Network): Promise<ApprovalProcessResponse>;
    upgradeContract(params: UpgradeContractRequest): Promise<UpgradeContractResponse>;
    getUpgradeApprovalProcess(network: Network): Promise<ApprovalProcessResponse>;
    getBlockExplorerApiKey(blockExplorerApiKeyId: string): Promise<BlockExplorerApiKeyResponse>;
    listBlockExplorerApiKeys(): Promise<BlockExplorerApiKeyResponse[]>;
    createBlockExplorerApiKey(params: CreateBlockExplorerApiKeyRequest): Promise<BlockExplorerApiKeyResponse>;
    updateBlockExplorerApiKey(blockExplorerApiKeyId: string, params: UpdateBlockExplorerApiKeyRequest): Promise<BlockExplorerApiKeyResponse>;
    removeBlockExplorerApiKey(blockExplorerApiKeyId: string): Promise<RemoveBlockExplorerApiKeyResponse>;
    getDeploymentVerification(contractNetwork: Pick<VerificationRequest, 'contractNetwork'>, contractAddress: Pick<VerificationRequest, 'contractAddress'>): Promise<Verification | undefined>;
}
//# sourceMappingURL=index.d.ts.map