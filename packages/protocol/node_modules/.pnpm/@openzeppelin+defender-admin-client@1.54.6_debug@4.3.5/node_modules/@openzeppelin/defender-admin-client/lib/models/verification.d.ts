import { Network } from '@openzeppelin/defender-base-client';
export type Address = string;
export interface VerificationRequest {
    artifactPayload?: string;
    artifactUri?: string;
    referenceUri?: string;
    solidityFilePath: string;
    contractName: string;
    contractAddress: Address;
    contractNetwork: Network;
}
export type Verification = {
    verificationId: string;
    artifactUri?: string;
    referenceUri?: string;
    solidityFilePath: string;
    contractName: string;
    contractAddress: Address;
    contractNetwork: Network;
    onChainSha256: string;
    providedSha256: string;
    lastVerifiedAt: string;
    matchType: 'NO_MATCH' | 'PARTIAL' | 'EXACT';
    providedBy: string;
    providedByType: 'USER_EMAIL' | 'API_KEY';
};
//# sourceMappingURL=verification.d.ts.map