import { ProposalResponse } from '..';
import { ExternalApiCreateProposalRequest } from './proposal';
export interface ExternalApiProposalResponse extends ExternalApiCreateProposalRequest {
    contractIds?: string[];
    contractId: string;
    proposalId: string;
    createdAt: string;
    isActive: boolean;
    isArchived: boolean;
}
export interface ProposalListPaginatedResponse {
    items: ProposalResponse[];
    next?: string;
}
//# sourceMappingURL=response.d.ts.map