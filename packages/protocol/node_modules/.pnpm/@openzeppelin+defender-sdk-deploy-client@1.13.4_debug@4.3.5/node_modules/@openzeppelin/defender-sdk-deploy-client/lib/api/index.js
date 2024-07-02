"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.DeployClient = void 0;
const lodash_1 = require("lodash");
const defender_sdk_base_client_1 = require("@openzeppelin/defender-sdk-base-client");
const deploy_1 = require("../utils/deploy");
const DEPLOYMENTS_PATH = '/deployments';
const UPGRADES_PATH = '/upgrades';
const BLOCKEXPLORER_API_KEY_PATH = '/block-explorer-api-key';
class DeployClient extends defender_sdk_base_client_1.BaseApiClient {
    getPoolId() {
        return process.env.DEFENDER_POOL_ID || 'us-west-2_94f3puJWv';
    }
    getPoolClientId() {
        return process.env.DEFENDER_POOL_CLIENT_ID || '40e58hbc7pktmnp9i26hh5nsav';
    }
    getApiUrl() {
        return process.env.DEFENDER_API_URL || 'https://defender-api.openzeppelin.com/v2/';
    }
    async deployContract(params) {
        if ((0, lodash_1.isEmpty)(params.artifactUri) && (0, lodash_1.isEmpty)(params.artifactPayload))
            throw new Error(`Missing artifact in deploy request. Either artifactPayload or artifactUri must be included in the request.`);
        if (params.artifactPayload) {
            params.artifactPayload = JSON.stringify((0, deploy_1.extractArtifact)(params));
        }
        return this.apiCall(async (api) => {
            return api.post(`${DEPLOYMENTS_PATH}`, params);
        });
    }
    async getDeployedContract(id) {
        return this.apiCall(async (api) => {
            return api.get(`${DEPLOYMENTS_PATH}/${id}`);
        });
    }
    async listDeployments() {
        return this.apiCall(async (api) => {
            return api.get(`${DEPLOYMENTS_PATH}`);
        });
    }
    async getDeployApprovalProcess(network) {
        return this.apiCall(async (api) => {
            return api.get(`${DEPLOYMENTS_PATH}/config/${network}`);
        });
    }
    async upgradeContract(params) {
        return this.apiCall(async (api) => {
            return api.post(`${UPGRADES_PATH}`, params);
        });
    }
    async getUpgradeApprovalProcess(network) {
        return this.apiCall(async (api) => {
            return api.get(`${UPGRADES_PATH}/config/${network}`);
        });
    }
    async getBlockExplorerApiKey(blockExplorerApiKeyId) {
        return this.apiCall(async (api) => {
            return api.get(`${BLOCKEXPLORER_API_KEY_PATH}/${blockExplorerApiKeyId}`);
        });
    }
    async listBlockExplorerApiKeys() {
        return this.apiCall(async (api) => {
            return api.get(`${BLOCKEXPLORER_API_KEY_PATH}`);
        });
    }
    async createBlockExplorerApiKey(params) {
        return this.apiCall(async (api) => {
            return api.post(`${BLOCKEXPLORER_API_KEY_PATH}`, params);
        });
    }
    async updateBlockExplorerApiKey(blockExplorerApiKeyId, params) {
        return this.apiCall(async (api) => {
            return api.put(`${BLOCKEXPLORER_API_KEY_PATH}/${blockExplorerApiKeyId}`, params);
        });
    }
    async removeBlockExplorerApiKey(blockExplorerApiKeyId) {
        return this.apiCall(async (api) => {
            return api.delete(`${BLOCKEXPLORER_API_KEY_PATH}/${blockExplorerApiKeyId}`);
        });
    }
    async getDeploymentVerification(contractNetwork, contractAddress) {
        return this.apiCall(async (api) => {
            try {
                return (await api.get(`/verifications/${contractNetwork}/${contractAddress}`));
            }
            catch {
                return undefined;
            }
        });
    }
}
exports.DeployClient = DeployClient;
