"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.AdminClient = void 0;
const defender_base_client_1 = require("@openzeppelin/defender-base-client");
const lodash_1 = require("lodash");
const utils_1 = require("ethers/lib/utils");
const utils_2 = require("./utils");
class AdminClient extends defender_base_client_1.BaseApiClient {
    getPoolId() {
        return process.env.DEFENDER_ADMIN_POOL_ID || 'us-west-2_94f3puJWv';
    }
    getPoolClientId() {
        return process.env.DEFENDER_ADMIN_POOL_CLIENT_ID || '40e58hbc7pktmnp9i26hh5nsav';
    }
    getApiUrl(v = 'v1') {
        if (v === 'v2') {
            return process.env.DEFENDER_API_V2_URL || 'https://defender-api.openzeppelin.com/v2/';
        }
        return process.env.DEFENDER_ADMIN_API_URL || 'https://defender-api.openzeppelin.com/admin/';
    }
    async addContract(contract) {
        return this.apiCall(async (api) => {
            return (await api.put('/contracts', contract));
        });
    }
    async deleteContract(contractId) {
        return this.apiCall(async (api) => {
            return (await api.delete(`/contracts/${contractId}`));
        });
    }
    async listContracts() {
        return this.apiCall(async (api) => {
            return (await api.get('/contracts'));
        });
    }
    // added separate from CreateProposalRequest type as the `simulate` boolean is contained within defender-client
    async createProposal(proposal) {
        return this.apiCall(async (api) => {
            let simulation = undefined;
            let simulationData = '0x';
            const isBatchProposal = (contract) => (0, lodash_1.isArray)(contract);
            // handle simulation checks before creating proposal
            if (proposal.simulate) {
                // we do not support simulating batch proposals from the client.
                if (isBatchProposal(proposal.contract)) {
                    throw new Error('Simulating a batch proposal is currently not supported from the API. Use the Defender UI to manually trigger a simulation.');
                }
                const overrideData = proposal.overrideSimulationOpts?.transactionData.data;
                simulationData = overrideData ?? '0x';
                // only check if we haven't overridden the simulation data property
                if (!overrideData) {
                    // Check if ABI is provided so we can encode the function
                    if (!proposal.contract.abi) {
                        // no ABI found, request user to pass in `data` in overrideSimulationOpts
                        throw new Error('Simulation requested without providing ABI. Please provide the contract ABI or use the `overrideSimulationOpts` to provide the data property directly.');
                    }
                    const contractInterface = new utils_1.Interface(proposal.contract.abi);
                    // this is defensive and should never happen since createProposal schema validation will fail without this property defined.
                    if (!proposal.functionInterface) {
                        // no function selected, request user to pass in `data` in overrideSimulationOpts
                        throw new Error('Simulation requested without providing function interface. Please provide the function interface or use the `overrideSimulationOpts` to provide the data property directly.');
                    }
                    simulationData = contractInterface.encodeFunctionData(proposal.functionInterface.name, proposal.functionInputs);
                }
            }
            // create proposal
            const response = (await api.post('/proposals', proposal));
            // create simulation
            if (proposal.simulate && !isBatchProposal(proposal.contract)) {
                try {
                    simulation = await this.simulateProposal(response.contractId, response.proposalId, {
                        transactionData: {
                            from: proposal.via,
                            to: proposal.contract.address,
                            data: simulationData,
                            value: proposal.metadata?.sendValue ?? '0',
                            ...proposal.overrideSimulationOpts?.transactionData,
                        },
                        blockNumber: proposal.overrideSimulationOpts?.blockNumber,
                    });
                }
                catch (e) {
                    // simply log so we don't block createProposal response
                    console.warn('Simulation Failed:', e);
                }
            }
            return { ...response, url: (0, utils_2.getProposalUrl)(response), simulation };
        });
    }
    async listProposals(opts = {}) {
        return this.apiCall(async (api) => {
            const response = (await api.get('/proposals', { params: opts }));
            if (Array.isArray(response)) {
                return response.map((proposal) => ({ ...proposal, url: (0, utils_2.getProposalUrl)(proposal) }));
            }
            return response;
        });
    }
    async getProposal(contractId, proposalId) {
        return this.apiCall(async (api) => {
            const response = (await api.get(`/contracts/${contractId}/proposals/${proposalId}`));
            return { ...response, url: (0, utils_2.getProposalUrl)(response) };
        });
    }
    async archiveProposal(_, proposalId) {
        return this.apiCall(async (api) => {
            const response = (await api.put(`proposals/archive/${proposalId}`));
            return { ...response, url: (0, utils_2.getProposalUrl)(response) };
        }, 'v2');
    }
    async unarchiveProposal(_, proposalId) {
        return this.apiCall(async (api) => {
            const response = (await api.put(`proposals/unarchive/${proposalId}`));
            return { ...response, url: (0, utils_2.getProposalUrl)(response) };
        }, 'v2');
    }
    async getProposalSimulation(contractId, proposalId) {
        return this.apiCall(async (api) => {
            const response = (await api.get(`/contracts/${contractId}/proposals/${proposalId}/simulation`));
            return response;
        });
    }
    async simulateProposal(contractId, proposalId, transaction) {
        return this.apiCall(async (api) => {
            const response = (await api.post(`/contracts/${contractId}/proposals/${proposalId}/simulate`, transaction));
            return response;
        });
    }
    async proposeUpgrade(params, contract) {
        const request = {
            contract,
            type: 'upgrade',
            metadata: {
                newImplementationAddress: params.newImplementation,
                newImplementationAbi: params.newImplementationAbi,
                proxyAdminAddress: params.proxyAdmin,
            },
            title: params.title ?? `Upgrade to ${params.newImplementation.slice(0, 10)}`,
            description: params.description ?? `Upgrade contract implementation to ${params.newImplementation}`,
            via: params.via,
            viaType: params.viaType,
            relayerId: params.relayerId,
        };
        return this.createProposal(request);
    }
    async proposePause(params, contract) {
        return this.proposePauseabilityAction(params, contract, 'pause');
    }
    async proposeUnpause(params, contract) {
        return this.proposePauseabilityAction(params, contract, 'unpause');
    }
    async proposeGrantRole(params, contract, role, account) {
        return this.proposeAccessControlAction(params, contract, 'grantRole', role, account);
    }
    async proposeRevokeRole(params, contract, role, account) {
        return this.proposeAccessControlAction(params, contract, 'revokeRole', role, account);
    }
    async verifyDeployment(params) {
        if ((0, lodash_1.isEmpty)(params.artifactUri) && ((0, lodash_1.isEmpty)(params.artifactPayload) || (0, lodash_1.isEmpty)(params.referenceUri)))
            throw new Error(`Missing artifact in verification request. Either artifactPayload and referenceUri, or artifactUri must be included in the request.`);
        return this.apiCall(async (api) => {
            return (await api.post('/verifications', params));
        });
    }
    async getDeploymentVerification(params) {
        return this.apiCall(async (api) => {
            try {
                return (await api.get(`/verifications/${params.contractNetwork}/${params.contractAddress}`));
            }
            catch {
                return undefined;
            }
        });
    }
    async proposePauseabilityAction(params, contract, action) {
        const request = {
            contract,
            type: 'pause',
            via: params.via,
            viaType: params.viaType,
            relayerId: params.relayerId,
            functionInputs: [],
            functionInterface: { name: action, inputs: [] },
            metadata: {
                action,
            },
            title: params.title ?? `${(0, lodash_1.capitalize)(action)} contract`,
            description: params.description ?? `${(0, lodash_1.capitalize)(action)} contract`,
        };
        return this.createProposal(request);
    }
    async proposeAccessControlAction(params, contract, action, role, account) {
        const request = {
            contract,
            type: 'access-control',
            via: params.via,
            viaType: params.viaType,
            relayerId: params.relayerId,
            functionInputs: [role, account],
            functionInterface: {
                name: action,
                inputs: [
                    {
                        name: 'role',
                        type: 'bytes32',
                    },
                    {
                        name: 'account',
                        type: 'address',
                    },
                ],
            },
            metadata: {
                action,
                role,
                account,
            },
            title: params.title ?? `${(0, lodash_1.capitalize)(action)} to ${account}`,
            description: params.description ?? `${(0, lodash_1.capitalize)(action)} to ${account}`,
        };
        return this.createProposal(request);
    }
}
exports.AdminClient = AdminClient;
