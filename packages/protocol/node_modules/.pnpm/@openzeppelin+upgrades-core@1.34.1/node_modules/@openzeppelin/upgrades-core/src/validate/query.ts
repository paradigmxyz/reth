import { Version, getVersion } from '../version';
import { ValidationRunData, ValidationError, isOpcodeError, ContractValidation } from './run';
import { StorageLayout } from '../storage/layout';
import { unlinkBytecode } from '../link-refs';
import { ValidationOptions, processExceptions } from './overrides';
import { ContractSourceNotFoundError, ValidationErrors } from './error';
import { ValidationData, normalizeValidationData } from './data';
import { ProxyDeployment } from '../manifest';

const upgradeToSignature = 'upgradeTo(address)';
const upgradeToAndCallSignature = 'upgradeToAndCall(address,bytes)';

export function assertUpgradeSafe(data: ValidationData, version: Version, opts: ValidationOptions): void {
  const dataV3 = normalizeValidationData(data);
  const [fullContractName] = getContractNameAndRunValidation(dataV3, version);

  const errors = getErrors(dataV3, version, opts);

  if (errors.length > 0) {
    throw new ValidationErrors(fullContractName, errors);
  }
}

/**
 * Gets the contract version object from the given validation run data and contract name (either fully qualified or simple contract name).
 *
 * @param runData The validation run data
 * @param contractName Fully qualified or simple contract name
 * @returns contract version object
 * @throws {Error} if the given contract name is not found or is ambiguous
 */
export function getContractVersion(runData: ValidationRunData, contractName: string): Version {
  let version = undefined;
  if (contractName.includes(':')) {
    version = runData[contractName].version;
  } else {
    const foundNames = Object.keys(runData).filter(element => element.endsWith(`:${contractName}`));
    if (foundNames.length > 1) {
      throw new Error(`Contract ${contractName} is ambiguous. Use one of the following:\n${foundNames.join('\n')}`);
    } else if (foundNames.length === 1) {
      version = runData[foundNames[0]].version;
    }
  }

  if (version === undefined) {
    throw new Error(`Contract ${contractName} is abstract`);
  }

  return version;
}

/**
 * Gets the fully qualified contract name and validation run data.
 *
 * @param data The validation data
 * @param version The contract Version
 * @returns fully qualified contract name and validation run data
 */
export function getContractNameAndRunValidation(data: ValidationData, version: Version): [string, ValidationRunData] {
  const dataV3 = normalizeValidationData(data);

  let runValidation;
  let fullContractName;

  for (const validation of dataV3.log) {
    fullContractName = Object.keys(validation).find(
      name => validation[name].version?.withMetadata === version.withMetadata,
    );
    if (fullContractName !== undefined) {
      runValidation = validation;
      break;
    }
  }

  if (fullContractName === undefined || runValidation === undefined) {
    throw new ContractSourceNotFoundError();
  }

  return [fullContractName, runValidation];
}

export function getStorageLayout(data: ValidationData, version: Version): StorageLayout {
  const dataV3 = normalizeValidationData(data);
  const [fullContractName, runValidation] = getContractNameAndRunValidation(dataV3, version);
  return unfoldStorageLayout(runValidation, fullContractName);
}

export function unfoldStorageLayout(runData: ValidationRunData, fullContractName: string): StorageLayout {
  const c = runData[fullContractName];
  const { solcVersion } = c;
  if (c.layout.flat) {
    return {
      solcVersion,
      storage: c.layout.storage,
      types: c.layout.types,
      namespaces: c.layout.namespaces,
    };
  } else {
    // Namespaces are pre-flattened
    const layout: StorageLayout = { solcVersion, storage: [], types: {}, namespaces: c.layout.namespaces };
    for (const name of [fullContractName].concat(c.inherit)) {
      layout.storage.unshift(...runData[name].layout.storage);
      Object.assign(layout.types, runData[name].layout.types);
    }
    return layout;
  }
}

export function* findVersionWithoutMetadataMatches(
  data: ValidationData,
  versionWithoutMetadata: string,
): Generator<[string, ValidationRunData]> {
  const dataV3 = normalizeValidationData(data);

  for (const validation of dataV3.log) {
    for (const contractName in validation) {
      if (validation[contractName].version?.withoutMetadata === versionWithoutMetadata) {
        yield [contractName, validation];
      }
    }
  }
}

export function getUnlinkedBytecode(data: ValidationData, bytecode: string): string {
  const dataV3 = normalizeValidationData(data);

  for (const validation of dataV3.log) {
    const linkableContracts = Object.keys(validation).filter(name => validation[name].linkReferences.length > 0);

    for (const name of linkableContracts) {
      const { linkReferences } = validation[name];
      const unlinkedBytecode = unlinkBytecode(bytecode, linkReferences);
      const version = getVersion(unlinkedBytecode);

      if (validation[name].version?.withMetadata === version.withMetadata) {
        return unlinkedBytecode;
      }
    }
  }

  return bytecode;
}

export function getErrors(data: ValidationData, version: Version, opts: ValidationOptions = {}): ValidationError[] {
  const dataV3 = normalizeValidationData(data);
  const [fullContractName, runValidation] = getContractNameAndRunValidation(dataV3, version);
  const c = runValidation[fullContractName];

  const errors = getAllErrors(runValidation, fullContractName);

  const selfAndInheritedMethods = getAllMethods(runValidation, fullContractName);

  if (
    !selfAndInheritedMethods.includes(upgradeToSignature) &&
    !selfAndInheritedMethods.includes(upgradeToAndCallSignature)
  ) {
    errors.push({
      src: c.src,
      kind: 'missing-public-upgradeto',
    });
  }

  return processExceptions(fullContractName, errors, opts);
}

function getAllErrors(runValidation: ValidationRunData, fullContractName: string) {
  // add self's opcode errors only, since opcode errors already include parents
  const opcodeErrors = runValidation[fullContractName].errors.filter(error => isOpcodeError(error));

  // add other errors from self and inherited contracts
  const otherErrors = getUsedContracts(fullContractName, runValidation)
    .flatMap(name => runValidation[name].errors)
    .filter(error => !isOpcodeError(error));

  return [...opcodeErrors, ...otherErrors];
}

function getAllMethods(runValidation: ValidationRunData, fullContractName: string): string[] {
  const c = runValidation[fullContractName];
  return c.methods.concat(...c.inherit.map(name => runValidation[name].methods));
}

function getUsedContracts(contractName: string, runValidation: ValidationRunData) {
  const c = runValidation[contractName];
  // Add current contract and all of its parents
  const res = new Set([contractName, ...c.inherit]);
  return Array.from(res);
}

export function isUpgradeSafe(data: ValidationData, version: Version): boolean {
  const dataV3 = normalizeValidationData(data);
  return getErrors(dataV3, version).length == 0;
}

export function inferUUPS(runValidation: ValidationRunData, fullContractName: string): boolean {
  const methods = getAllMethods(runValidation, fullContractName);
  return methods.includes(upgradeToSignature) || methods.includes(upgradeToAndCallSignature);
}

export function inferProxyKind(data: ValidationData, version: Version): ProxyDeployment['kind'] {
  const dataV3 = normalizeValidationData(data);
  const [fullContractName, runValidation] = getContractNameAndRunValidation(dataV3, version);
  if (inferUUPS(runValidation, fullContractName)) {
    return 'uups';
  } else {
    return 'transparent';
  }
}

/**
 * Whether the contract inherits any contract named "Initializable"
 * @param contractValidation The validation result for the contract
 * @return true if the contract inheritss any contract whose fully qualified name ends with ":Initializable"
 */
export function inferInitializable(contractValidation: ContractValidation) {
  return contractValidation.inherit.some(c => c.endsWith(':Initializable'));
}
