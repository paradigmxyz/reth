import { EthereumProvider, getBeaconAddress, isBeaconProxy, isTransparentOrUUPSProxy, UpgradesError } from '.';

export class BeaconProxyUnsupportedError extends UpgradesError {
  constructor() {
    super(
      'Beacon proxies are not supported with the current function.',
      () => 'Use deployBeacon(), deployBeaconProxy(), or upgradeBeacon() instead.',
    );
  }
}

export class DeployBeaconProxyKindError extends UpgradesError {
  constructor(kind: string) {
    super(
      `Unsupported proxy kind '${kind}'`,
      () => `deployBeaconProxy() is only supported with proxy kind undefined or 'beacon'`,
    );
  }
}

export class DeployBeaconProxyUnsupportedError extends UpgradesError {
  constructor(beaconAddress: string) {
    super(`Contract at ${beaconAddress} doesn't look like a beacon`, () => 'Deploy a beacon using deployBeacon().');
  }
}

export class DeployBeaconProxyImplUnknownError extends UpgradesError {
  constructor(implAddress: string) {
    super(
      `Beacon's current implementation at ${implAddress} is unknown`,
      () => `Call deployBeaconProxy() with the implementation option providing the beacon's current implementation.`,
    );
  }
}

export class LoadProxyUnsupportedError extends UpgradesError {
  constructor(proxyAddress: string) {
    super(
      `Contract at ${proxyAddress} doesn't look like a supported proxy`,
      () => 'Only transparent, UUPS, or beacon proxies can be loaded with the loadProxy() function.',
    );
  }
}

/**
 * @deprecated No longer used since prepareUpgrade() supports using an implementation contract as the reference address.
 */
export class PrepareUpgradeUnsupportedError extends UpgradesError {
  constructor(proxyOrBeaconAddress: string) {
    super(
      `Contract at address ${proxyOrBeaconAddress} doesn't look like a supported proxy or beacon`,
      () => `Only transparent, UUPS, or beacon proxies or beacons can be used with the prepareUpgrade() function.`,
    );
  }
}

/**
 * @deprecated No longer used since forceImport() supports importing any contract.
 */
export class ForceImportUnsupportedError extends UpgradesError {
  constructor(proxyOrBeaconAddress: string) {
    super(
      `Contract at address ${proxyOrBeaconAddress} doesn't look like a supported proxy or beacon`,
      () => `Only transparent, UUPS, or beacon proxies or beacons can be used with the forceImport() function.`,
    );
  }
}

export class NoContractImportError extends UpgradesError {
  constructor(address: string) {
    super(
      `No contract at address ${address}`,
      () => `The address could not be imported because no contract was found at the address.`,
    );
  }
}

export class ValidateUpdateRequiresKindError extends UpgradesError {
  constructor() {
    super(
      'The `kind` option must be provided',
      () =>
        'When validating an upgrade from an implementation address, pass in the `kind` option for the kind of proxy that you are using.',
    );
  }
}

export class PrepareUpgradeRequiresKindError extends UpgradesError {
  constructor() {
    super(
      'The `kind` option must be provided',
      () =>
        'When preparing an upgrade from an implementation address, pass in the `kind` option for the kind of proxy that you are using.',
    );
  }
}

export class InitialOwnerUnsupportedKindError extends UpgradesError {
  constructor(kind: string) {
    super(
      `The \`initialOwner\` option is not supported for this kind of proxy ('${kind}')`,
      () => `Set the initial owner as part of your contract's initializer arguments instead.`,
    );
  }
}

export async function assertNotProxy(provider: EthereumProvider, address: string) {
  if (await isTransparentOrUUPSProxy(provider, address)) {
    throw new UpgradesError(
      'Address is a transparent or UUPS proxy which cannot be upgraded using upgradeBeacon().',
      () => 'Use upgradeProxy() instead.',
    );
  } else if (await isBeaconProxy(provider, address)) {
    const beaconAddress = await getBeaconAddress(provider, address);
    throw new UpgradesError(
      'Address is a beacon proxy which cannot be upgraded directly.',
      () =>
        `upgradeBeacon() must be called with a beacon address, not a beacon proxy address. Call upgradeBeacon() on the beacon address ${beaconAddress} instead.`,
    );
  }
}
