"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.getProposalUrl = void 0;
const defender_base_client_1 = require("@openzeppelin/defender-base-client");
function getProposalUrl(proposal) {
    return `${defender_base_client_1.DEFENDER_APP_URL}/#/admin/contracts/${proposal.contractId}/proposals/${proposal.proposalId}`;
}
exports.getProposalUrl = getProposalUrl;
