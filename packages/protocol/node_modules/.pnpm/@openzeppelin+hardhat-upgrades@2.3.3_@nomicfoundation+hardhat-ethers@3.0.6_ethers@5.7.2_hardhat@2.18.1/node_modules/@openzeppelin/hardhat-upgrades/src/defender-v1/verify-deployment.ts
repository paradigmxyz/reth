import '../type-extensions';
import { AdminClient, VerificationRequest } from '@openzeppelin/defender-admin-client';
import { Artifact, BuildInfo, CompilerOutputBytecode, HardhatRuntimeEnvironment } from 'hardhat/types';
import { getAdminClient, getNetwork } from './utils';
import { sha256FromString } from 'ethereumjs-util';

export type VerificationResponse = Awaited<ReturnType<AdminClient['verifyDeployment']>>;

type ExtendedArtifact = Artifact & { immutableReferences: CompilerOutputBytecode['immutableReferences'] };

export type VerifyDeployFunction = (
  address: string,
  contractName: string,
  referenceUrl: string,
) => Promise<VerificationResponse>;

export type VerifyDeployWithUploadedArtifactFunction = (
  address: string,
  contractName: string,
  artifactUri: string,
) => Promise<VerificationResponse>;

export type GetVerifyDeployArtifactFunction = (contractName: string) => Promise<ExtendedArtifact>;

export type GetVerifyDeployBuildInfoFunction = (contractName: string) => Promise<BuildInfo>;

export type GetBytecodeDigestFunction = (contractName: string) => Promise<string>;

export function makeVerifyDeploy(hre: HardhatRuntimeEnvironment): VerifyDeployFunction {
  return async function verifyDeploy(address, contractName, referenceUri) {
    const client = getAdminClient(hre);
    const contractNetwork = await getNetwork(hre);
    const artifact = await getExtendedArtifact(hre, contractName);

    const request: VerificationRequest = {
      contractAddress: address,
      contractName: artifact.contractName,
      solidityFilePath: artifact.sourceName,
      contractNetwork,
      artifactPayload: JSON.stringify(artifact),
      referenceUri,
    };

    return client.verifyDeployment(request);
  };
}

export function makeVerifyDeployWithUploadedArtifact(
  hre: HardhatRuntimeEnvironment,
): VerifyDeployWithUploadedArtifactFunction {
  return async function verifyDeploy(address, contractName, artifactUri) {
    const client = getAdminClient(hre);
    const contractNetwork = await getNetwork(hre);
    const artifact = await hre.artifacts.readArtifact(contractName);

    const request: VerificationRequest = {
      contractAddress: address,
      contractName: artifact.contractName,
      solidityFilePath: artifact.sourceName,
      artifactUri,
      contractNetwork,
    };

    return client.verifyDeployment(request);
  };
}

export function makeGetVerifyDeployArtifact(hre: HardhatRuntimeEnvironment): GetVerifyDeployArtifactFunction {
  return async function getVerifyDeployArtifact(contractName: string): Promise<ExtendedArtifact> {
    return getExtendedArtifact(hre, contractName);
  };
}

export function makeGetVerifyDeployBuildInfo(hre: HardhatRuntimeEnvironment): GetVerifyDeployBuildInfoFunction {
  return async function getVerifyDeployBuildInfo(contractName: string): Promise<BuildInfo> {
    const artifact = await hre.artifacts.readArtifact(contractName);
    const fqn = `${artifact.sourceName}:${artifact.contractName}`;
    const buildInfo = await hre.artifacts.getBuildInfo(fqn);
    if (!buildInfo) {
      throw new Error(`Build info for ${fqn} not found`);
    }
    return buildInfo;
  };
}

export function makeGetBytecodeDigest(hre: HardhatRuntimeEnvironment) {
  return async function getBytecodeDigest(contractName: string): Promise<string> {
    const artifact = await hre.artifacts.readArtifact(contractName);
    const bytecode = artifact.deployedBytecode;
    return sha256FromString(bytecode).toString('hex');
  };
}

async function getExtendedArtifact(hre: HardhatRuntimeEnvironment, contractName: string): Promise<ExtendedArtifact> {
  const artifact = await hre.artifacts.readArtifact(contractName);
  const fqn = `${artifact.sourceName}:${artifact.contractName}`;
  const buildInfo = await hre.artifacts.getBuildInfo(fqn);
  const contractBuildInfo = buildInfo?.output.contracts[artifact.sourceName][artifact.contractName];
  const immutableReferences = contractBuildInfo?.evm.deployedBytecode.immutableReferences ?? {};
  return { ...artifact, immutableReferences };
}
