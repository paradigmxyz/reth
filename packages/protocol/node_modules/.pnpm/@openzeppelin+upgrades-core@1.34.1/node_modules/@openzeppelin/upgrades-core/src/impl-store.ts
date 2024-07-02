import debug from './utils/debug';
import { Manifest, ManifestData, ImplDeployment } from './manifest';
import { EthereumProvider, isDevelopmentNetwork } from './provider';
import {
  Deployment,
  RemoteDeploymentId,
  RemoteDeployment,
  InvalidDeployment,
  resumeOrDeploy,
  waitAndValidateDeployment,
} from './deployment';
import { Version } from './version';
import assert from 'assert';
import { DeployOpts } from '.';

interface ManifestLens<T> {
  description: string;
  type: string;
  (data: ManifestData): ManifestField<T>;
}

export interface ManifestField<T> {
  get(): T | undefined;
  set(value: T | undefined): void;
  merge?(value: T | undefined): void;
}

/**
 * Fetches the deployment from the manifest, or deploys it if not found.
 *
 * @param lens the manifest lens
 * @param provider the Ethereum provider
 * @param deploy the deploy function
 * @param opts options containing the timeout and pollingInterval parameters. If undefined, assumes the timeout is not configurable and will not mention those parameters in the error message for TransactionMinedTimeout.
 * @param merge if true, adds a deployment to existing deployment by merging their addresses. Defaults to false.
 * @param getRemoteDeployment a function to get the remote deployment status by id. If the deployment id is not found, returns undefined.
 * @returns the deployment
 * @throws {InvalidDeployment} if the deployment is invalid
 * @throws {TransactionMinedTimeout} if the transaction was not confirmed within the timeout period
 */
async function fetchOrDeployGeneric<T extends Deployment, U extends T = T>(
  lens: ManifestLens<T>,
  provider: EthereumProvider,
  deploy: () => Promise<U>,
  opts?: DeployOpts,
  merge?: boolean,
  getRemoteDeployment?: (remoteDeploymentId: string) => Promise<RemoteDeployment | undefined>,
): Promise<U | Deployment> {
  const manifest = await Manifest.forNetwork(provider);

  try {
    const deployment = await manifest.lockedRun(async () => {
      debug('fetching deployment of', lens.description);
      const data = await manifest.read();
      const deployment = lens(data);
      if (merge && !deployment.merge) {
        throw new Error(
          'fetchOrDeployGeneric was called with merge set to true but the deployment lens does not have a merge function',
        );
      }

      const stored = deployment.get();
      const updated = await resumeOrDeploy(
        provider,
        stored,
        deploy,
        lens.type,
        opts,
        deployment,
        merge,
        getRemoteDeployment,
      );
      if (updated !== stored) {
        if (merge && deployment.merge) {
          await checkForAddressClash(provider, data, updated, true);
          deployment.merge(updated);
        } else {
          await checkForAddressClash(provider, data, updated, false);
          deployment.set(updated);
        }
        await manifest.write(data);
      }
      return updated;
    });

    await waitAndValidateDeployment(provider, deployment, lens.type, opts, getRemoteDeployment);

    return deployment;
  } catch (e) {
    // If we run into a deployment error, we remove it from the manifest.
    if (e instanceof InvalidDeployment) {
      await manifest.lockedRun(async () => {
        assert(e instanceof InvalidDeployment); // Not sure why this is needed but otherwise doesn't type
        const data = await manifest.read();
        const deployment = lens(data);
        const stored = deployment.get();
        if (stored?.txHash === e.deployment.txHash) {
          deployment.set(undefined);
          await manifest.write(data);
        }
      });
      e.removed = true;
    }

    throw e;
  }
}

/**
 * Deletes the deployment by setting it to undefined.
 * Should only be used during a manifest run.
 */
export function deleteDeployment(deployment: ManifestField<Deployment>) {
  deployment.set(undefined);
}

/**
 * Fetches the deployment address from the manifest, or deploys it if not found and returns the address.
 *
 * @param version the contract version
 * @param provider the Ethereum provider
 * @param deploy the deploy function
 * @param opts options containing the timeout and pollingInterval parameters. If undefined, assumes the timeout is not configurable and will not mention those parameters in the error message for TransactionMinedTimeout.
 * @param merge if true, adds a deployment to existing deployment by merging their addresses. Defaults to false.
 * @returns the deployment address
 * @throws {InvalidDeployment} if the deployment is invalid
 * @throws {TransactionMinedTimeout} if the transaction was not confirmed within the timeout period
 */
export async function fetchOrDeploy(
  version: Version,
  provider: EthereumProvider,
  deploy: () => Promise<ImplDeployment>,
  opts?: DeployOpts,
  merge?: boolean,
): Promise<string> {
  return (await fetchOrDeployGeneric(implLens(version.linkedWithoutMetadata), provider, deploy, opts, merge)).address;
}

/**
 * Fetches the deployment from the manifest, or deploys it if not found and returns the deployment.
 *
 * @param version the contract version
 * @param provider the Ethereum provider
 * @param deploy the deploy function
 * @param opts options containing the timeout and pollingInterval parameters. If undefined, assumes the timeout is not configurable and will not mention those parameters in the error message for TransactionMinedTimeout.
 * @param merge if true, adds a deployment to existing deployment by merging their addresses. Defaults to false.
 * @param getRemoteDeployment a function to get the remote deployment status by id. If the deployment id is not found, returns undefined.
 * @returns the deployment
 * @throws {InvalidDeployment} if the deployment is invalid
 * @throws {TransactionMinedTimeout} if the transaction was not confirmed within the timeout period
 */
export async function fetchOrDeployGetDeployment<T extends ImplDeployment>(
  version: Version,
  provider: EthereumProvider,
  deploy: () => Promise<T>,
  opts?: DeployOpts,
  merge?: boolean,
  getRemoteDeployment?: (remoteDeploymentId: string) => Promise<RemoteDeployment | undefined>,
): Promise<T | Deployment> {
  return fetchOrDeployGeneric(
    implLens(version.linkedWithoutMetadata),
    provider,
    deploy,
    opts,
    merge,
    getRemoteDeployment,
  );
}

const implLens = (versionWithoutMetadata: string) =>
  lens(`implementation ${versionWithoutMetadata}`, 'implementation', data => ({
    get: () => data.impls[versionWithoutMetadata],
    set: (value?: ImplDeployment & RemoteDeploymentId) => (data.impls[versionWithoutMetadata] = value),
    merge: (value?: ImplDeployment & RemoteDeploymentId) => {
      const existing = data.impls[versionWithoutMetadata];
      if (existing !== undefined && value !== undefined) {
        const { address, allAddresses } = mergeAddresses(existing, value);
        data.impls[versionWithoutMetadata] = {
          ...existing,
          address,
          allAddresses,
          remoteDeploymentId: value.remoteDeploymentId,
        };
      } else {
        data.impls[versionWithoutMetadata] = value;
      }
    },
  }));

/**
 * Merge the addresses in the deployments and returns them.
 *
 * @param existing existing deployment
 * @param value deployment to add
 */
export function mergeAddresses(existing: ImplDeployment, value: ImplDeployment) {
  const merged = new Set<string>();

  merged.add(existing.address);
  merged.add(value.address);

  existing.allAddresses?.forEach(item => merged.add(item));
  value.allAddresses?.forEach(item => merged.add(item));

  return { address: existing.address, allAddresses: Array.from(merged) };
}

export async function fetchOrDeployAdmin(
  provider: EthereumProvider,
  deploy: () => Promise<Deployment>,
  opts?: DeployOpts,
): Promise<string> {
  return (await fetchOrDeployGeneric(adminLens, provider, deploy, opts)).address;
}

const adminLens = lens('proxy admin', 'proxy admin', data => ({
  get: () => data.admin,
  set: (value?: Deployment) => (data.admin = value),
}));

function lens<T>(description: string, type: string, fn: (data: ManifestData) => ManifestField<T>): ManifestLens<T> {
  return Object.assign(fn, { description, type });
}

async function checkForAddressClash(
  provider: EthereumProvider,
  data: ManifestData,
  updated: Deployment & RemoteDeploymentId,
  merge: boolean,
): Promise<void> {
  let clash;
  const findClash = () => lookupDeployment(data, updated.address, !merge);

  if (await isDevelopmentNetwork(provider)) {
    // Look for clashes so that we can delete deployments from older runs.
    // `merge` only checks primary addresses for clashes, since the address could already exist in an allAddresses field
    // but the updated and stored objects are different instances representing the same entry.
    clash = findClash();
    if (clash !== undefined) {
      debug('deleting a previous deployment at address', updated.address);
      clash.set(undefined);
    }
  } else if (!merge) {
    clash = findClash();
    if (clash !== undefined) {
      const existing = clash.get();
      // it's a clash if there is no deployment id or if deployment ids don't match
      if (
        existing !== undefined &&
        (existing.remoteDeploymentId === undefined || existing.remoteDeploymentId !== updated.remoteDeploymentId)
      ) {
        throw new Error(
          `The deployment clashes with an existing one at ${updated.address}.\n\n` +
            `Deployments: ${JSON.stringify({ existing, updated }, null, 2)}\n\n`,
        );
      }
    }
  } // else, merge indicates that the user is force-importing or redeploying an implementation, so we simply allow merging the entries
}

function lookupDeployment(
  data: ManifestData,
  address: string,
  checkAllAddresses: boolean,
): ManifestField<Deployment & RemoteDeploymentId> | undefined {
  if (data.admin?.address === address) {
    return adminLens(data);
  }

  for (const versionWithoutMetadata in data.impls) {
    if (
      data.impls[versionWithoutMetadata]?.address === address ||
      (checkAllAddresses && data.impls[versionWithoutMetadata]?.allAddresses?.includes(address))
    ) {
      return implLens(versionWithoutMetadata)(data);
    }
  }
}
