import { EthereumProvider } from './provider';
import { inferProxyKind, ValidationData, ValidationOptions } from './validate';
import { Manifest, DeploymentNotFound, ProxyDeployment } from './manifest';
import { isBeaconProxy, isTransparentOrUUPSProxy, isTransparentProxy } from './eip-1967-type';
import { Version } from './version';
import { BeaconProxyUnsupportedError } from './usage-error';
import { UpgradesError } from '.';

export async function setProxyKind(
  provider: EthereumProvider,
  proxyAddress: string,
  opts: ValidationOptions,
): Promise<ProxyDeployment['kind']> {
  const manifest = await Manifest.forNetwork(provider);

  const manifestDeployment = await manifest.getProxyFromAddress(proxyAddress).catch(e => {
    if (e instanceof DeploymentNotFound) {
      return undefined;
    } else {
      throw e;
    }
  });

  if (opts.kind === undefined) {
    opts.kind = manifestDeployment?.kind ?? 'transparent';
  } else if (manifestDeployment && opts.kind !== manifestDeployment.kind) {
    throw new Error(`Requested an upgrade of kind ${opts.kind} but proxy is ${manifestDeployment.kind}`);
  }

  return opts.kind;
}

/**
 * Processes opts.kind when deploying the implementation for a UUPS or Transparent proxy.
 *
 * @throws {BeaconProxyUnsupportedError} If this function is called for a Beacon proxy.
 */
export async function processProxyKind(
  provider: EthereumProvider,
  proxyAddress: string | undefined,
  opts: ValidationOptions,
  data: ValidationData,
  version: Version,
) {
  if (opts.kind === undefined) {
    if (proxyAddress !== undefined && (await isBeaconProxy(provider, proxyAddress))) {
      opts.kind = 'beacon';
    } else {
      opts.kind = inferProxyKind(data, version);
    }
  }

  if (proxyAddress !== undefined) {
    await setProxyKind(provider, proxyAddress, opts);
  }

  if (opts.kind === 'beacon') {
    throw new BeaconProxyUnsupportedError();
  }
}

/**
 * Detects the kind of proxy at an address by reading its ERC 1967 storage slots.
 *
 * @deprecated Not reliable since UUPS proxies can have admin storage slot set, which causes
 * this function to treat it as transparent.  Instead, if implementation contract signatures are
 * available, infer the proxy kind using `inferProxyKind` instead.
 *
 * @param provider the Ethereum provider
 * @param proxyAddress the proxy address
 * @returns the proxy kind
 * @throws {UpgradesError} if the contract at address does not look like an ERC 1967 proxy
 */
export async function detectProxyKind(provider: EthereumProvider, proxyAddress: string) {
  let importKind: ProxyDeployment['kind'];
  if (await isTransparentProxy(provider, proxyAddress)) {
    importKind = 'transparent';
  } else if (await isTransparentOrUUPSProxy(provider, proxyAddress)) {
    importKind = 'uups';
  } else if (await isBeaconProxy(provider, proxyAddress)) {
    importKind = 'beacon';
  } else {
    throw new UpgradesError(`Contract at ${proxyAddress} doesn't look like an ERC 1967 proxy`);
  }
  return importKind;
}
