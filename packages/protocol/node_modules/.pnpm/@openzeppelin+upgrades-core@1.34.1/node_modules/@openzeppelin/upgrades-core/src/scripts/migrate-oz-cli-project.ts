import path from 'path';
import chalk from 'chalk';
import { promises as fs } from 'fs';
import { compare as compareVersions } from 'compare-versions';
import { ManifestData, ImplDeployment, ProxyDeployment } from '../manifest';
import type { StorageItem, StorageLayout, TypeItem, TypeItemMembers, StructMember } from '../storage/layout';

const OPEN_ZEPPELIN_FOLDER = '.openzeppelin';
const EXPORT_FILE = 'openzeppelin-cli-export.json';
const PROJECT_FILE = path.join(OPEN_ZEPPELIN_FOLDER, 'project.json');
const SUCCESS_CHECK = chalk.green('✔') + ' ';

export async function migrateLegacyProject(): Promise<void> {
  const manifestFiles = await getManifestFiles();
  const networksExportData = await migrateManifestFiles(manifestFiles);

  const { compiler } = await getProjectFile();
  const exportData = {
    networks: networksExportData,
    compiler,
  };

  await writeJSONFile(EXPORT_FILE, exportData);
  console.log(SUCCESS_CHECK + `Migration data exported to ${EXPORT_FILE}`);
  await deleteLegacyFiles(manifestFiles);

  console.log("\nThese were your project's compiler options:");
  console.log(JSON.stringify(compiler, null, 2));
}

export async function migrateManifestFiles(manifestFiles: string[]): Promise<Record<string, NetworkExportData>> {
  const migratableManifestFiles = manifestFiles.filter(manifest => !isDevelopmentNetwork(getNetworkName(manifest)));
  const migratableManifestsData: Record<string, NetworkFileData> = {};

  for (const migratableFile of migratableManifestFiles) {
    const network = getNetworkName(migratableFile);
    migratableManifestsData[network] = JSON.parse(await fs.readFile(migratableFile, 'utf8'));
  }

  // we run the entire data migration before writing anything to disk
  const { newManifestsData, networksExportData } = migrateManifestsData(migratableManifestsData);

  for (const network in newManifestsData) {
    const newManifestData = newManifestsData[network];
    const newFilename = getNewManifestLocation(network);
    await writeJSONFile(newFilename, newManifestData);
    console.log(SUCCESS_CHECK + `Successfully migrated ${newFilename}`);
  }

  return networksExportData;
}

export async function deleteLegacyFiles(manifestFiles: string[]): Promise<void> {
  const developmentManifests = manifestFiles.filter(manifestFile => isDevelopmentNetwork(getNetworkName(manifestFile)));

  for (const manifestFile of developmentManifests) {
    console.log(SUCCESS_CHECK + `Deleting unused development manifest ${manifestFile}`);
    await fs.unlink(manifestFile);
  }

  console.log(SUCCESS_CHECK + `Deleting ${PROJECT_FILE}`);
  await fs.unlink(PROJECT_FILE);
}

export function migrateManifestsData(manifestsData: Record<string, NetworkFileData>): MigrationOutput {
  const networksExportData: Record<string, NetworkExportData> = {};
  const newManifestsData: Record<string, ManifestData> = {};

  for (const network of Object.keys(manifestsData)) {
    const oldManifestData = manifestsData[network];
    const { manifestVersion, zosversion } = oldManifestData;

    const currentVersion = manifestVersion ?? zosversion;

    if (currentVersion === undefined) {
      throw new Error('Migration failed: manifest version too old. Update your OpenZeppelin CLI version.');
    }

    if (compareVersions(currentVersion, '3.0', '>=')) {
      // no need to migrate
      continue;
    }

    if (currentVersion !== '2.2') {
      throw new Error(
        `Migration failed: expected manifest version 2.2, got ${currentVersion} instead. Update your OpenZeppelin CLI version.`,
      );
    }

    newManifestsData[network] = updateManifestData(oldManifestData);
    networksExportData[network] = getExportData(oldManifestData);
  }

  return {
    newManifestsData,
    networksExportData,
  };
}

async function getManifestFiles(): Promise<string[]> {
  const files = await fs.readdir(OPEN_ZEPPELIN_FOLDER);
  return files.filter(isManifestFile).map(location => path.join(OPEN_ZEPPELIN_FOLDER, location));
}

async function getProjectFile(): Promise<ProjectFileData> {
  return JSON.parse(await fs.readFile(PROJECT_FILE, 'utf8'));
}

function isManifestFile(filename: string): boolean {
  const network = getNetworkName(filename);
  return isPublicNetwork(network) || isDevelopmentNetwork(network) || isUnknownNetwork(network);
}

function getNetworkName(filename: string): string {
  return path.basename(filename, '.json');
}

function isDevelopmentNetwork(network: string): boolean {
  // 13+ digits      => ganache timestamp
  // 31337           => hardhat network
  return /^dev-(31337|\d{13,})$/.test(network);
}

function isUnknownNetwork(network: string): boolean {
  return !isDevelopmentNetwork(network) && /^dev-\d+$/.test(network);
}

function isPublicNetwork(network: string): boolean {
  return ['mainnet', 'rinkeby', 'ropsten', 'kovan', 'goerli'].includes(network);
}

function getNewManifestLocation(oldName: string): string {
  const filename = isUnknownNetwork(oldName) ? oldName.replace('dev', 'unknown') : oldName;
  return path.join(OPEN_ZEPPELIN_FOLDER, `${filename}.json`);
}

async function writeJSONFile(location: string, data: unknown): Promise<void> {
  await fs.writeFile(location, JSON.stringify(data, null, 2));
}

function getExportData(oldManifestData: NetworkFileData): NetworkExportData {
  const networksExportData: NetworkExportData & Partial<NetworkFileData> = Object.assign({}, oldManifestData);
  delete networksExportData.proxyAdmin;
  delete networksExportData.contracts;
  delete networksExportData.solidityLibs;
  delete networksExportData.manifestVersion;
  delete networksExportData.zosversion;
  delete networksExportData.version;
  delete networksExportData.frozen;
  return networksExportData;
}

function updateManifestData(oldManifestData: NetworkFileData): ManifestData {
  const proxyAdmin = oldManifestData.proxyAdmin.address;

  if (proxyAdmin === undefined) {
    throw new Error('Legacy manifest does not have admin address');
  }

  if (Object.keys(oldManifestData.solidityLibs).length > 0) {
    throw new Error('Legacy manifest links to external libraries which are not yet supported');
  }

  return {
    manifestVersion: '3.2',
    impls: transformImplementations(oldManifestData.contracts),
    proxies: [...transformProxies(oldManifestData.proxies)],
    admin: {
      address: proxyAdmin,
    },
  };
}

function* transformProxies(proxies: LegacyProxies): Generator<ProxyDeployment> {
  for (const contractName in proxies) {
    for (const proxy of proxies[contractName]) {
      switch (proxy.kind) {
        case 'Upgradeable':
          if (proxy.address) {
            yield {
              address: proxy.address,
              kind: 'transparent',
            };
          }
          break;

        case 'Minimal':
        case 'NonProxy':
          // not supported by the new plugin
          break;
      }
    }
  }
}

function transformImplementations(contracts: LegacyContracts): Record<string, ImplDeployment> {
  const impls: Record<string, ImplDeployment> = {};
  for (const contractName in contracts) {
    const contract = contracts[contractName];
    if (contract.deployedBytecodeHash === undefined) {
      continue;
    } else {
      impls[contract.deployedBytecodeHash] = transformImplementationItem(contract);
    }
  }
  return impls;
}

function transformImplementationItem(contract: ContractInterface): ImplDeployment {
  if (contract.address === undefined) {
    throw new Error('Could not find implementation address');
  }

  return {
    address: contract.address,
    layout: transformLayout(contract),
  };
}

function transformLayout(contract: ContractInterface): StorageLayout {
  const { types, storage } = contract;
  if (types === undefined || storage === undefined) {
    throw new Error("Storage layout can't be undefined");
  }

  // We need to associate a made up astId to some types since the OpenZeppelin CLI used to drop them
  const astIds = Object.keys(types);
  const getAstId = (typeName: string): number => {
    const astId = astIds.indexOf(typeName);
    if (astId === -1) {
      throw new Error(`Could not find type ${typeName}`);
    }
    return astId;
  };

  return {
    storage: storage.map((storageItem: LegacyStorageItem) => transformStorageItem(storageItem, getAstId)),
    types: transformTypes(types, getAstId),
  };
}

function transformStorageItem(storageItem: LegacyStorageItem, getAstId: AstIdGetter): StorageItem {
  return {
    contract: storageItem.contract,
    label: storageItem.label,
    type: transformTypeName(storageItem.type, getAstId),
    // TODO reconstruct path and line if sourcecode is available
    src: storageItem.path,
  };
}

function transformTypes(oldTypes: LegacyTypes, getAstId: AstIdGetter): Record<string, TypeItem> {
  const newTypes: Record<string, TypeItem> = {};
  for (const typeName in oldTypes) {
    newTypes[transformTypeName(typeName, getAstId)] = transformType(
      getTypeKind(typeName),
      oldTypes[typeName],
      getAstId,
    );
  }
  return newTypes;
}

function transformType(typeKind: TypeKind, oldType: LegacyType, getAstId: AstIdGetter): TypeItem {
  switch (typeKind) {
    case 'Struct':
      return {
        label: stripContractName(oldType.label),
        members: (oldType.members as StructMember[]).map(member => ({
          label: stripContractName(member.label),
          type: transformTypeName(member.type, getAstId),
        })),
      };
    case 'Enum':
      return {
        label: stripContractName(oldType.label),
        members: oldType.members,
      };
    default:
      return {
        label: stripContractName(oldType.label),
      };
  }
}

function transformTypeName(typeName: string, getAstId: AstIdGetter): string {
  switch (getTypeKind(typeName)) {
    case 'Struct':
      return transformStructTypeName(typeName, getAstId);
    case 'Enum':
      return transformEnumTypeName(typeName, getAstId);
    case 'Mapping':
      return transformMappingTypeName(typeName, getAstId);
    case 'DynArray':
      return transformDynArrayTypeName(typeName, getAstId);
    case 'StaticArray':
      return transformStaticArrayTypeName(typeName, getAstId);
    case 'Elementary':
      return typeName;
    default:
      throw new Error(`Unknown type: ${typeName}`);
  }
}

function transformStructTypeName(typeName: string, getAstId: AstIdGetter): string {
  const name = stripContractName(getArgument(typeName));
  const astId = getAstId(typeName);
  return `t_struct(${name})${astId}_storage`;
}

function transformEnumTypeName(typeName: string, getAstId: AstIdGetter): string {
  const name = stripContractName(getArgument(typeName));
  const astId = getAstId(typeName);
  return `t_enum(${name})${astId}`;
}

function transformMappingTypeName(typeName: string, getAstId: AstIdGetter): string {
  const valueType = transformTypeName(getArgument(typeName), getAstId);
  return `t_mapping(unknown,${valueType})`;
}

function transformDynArrayTypeName(typeName: string, getAstId: AstIdGetter): string {
  const valueType = transformTypeName(getArgument(typeName), getAstId);
  return `t_array(${valueType})dyn_storage`;
}

function transformStaticArrayTypeName(typeName: string, getAstId: AstIdGetter): string {
  // here we assume the regex has been already validated
  // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
  const size = typeName.match(/:(\d+)/)![1];
  const valueType = transformTypeName(getArgument(typeName), getAstId);
  return `t_array(${valueType})${size}_storage`;
}

function getArgument(typeName: string): string {
  // here we assume the regex has been already validated
  // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
  return typeName.match(/<(.+)>/)![1];
}

function stripContractName(s: string): string {
  // regex always matches
  // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
  return s.match(/(.+\.)?(.+)/)![2];
}

function getTypeKind(typeName: string): TypeKind {
  if (/^t_struct<.+>/.test(typeName)) {
    return 'Struct';
  } else if (/^t_enum<.+>/.test(typeName)) {
    return 'Enum';
  } else if (/^t_mapping<.+>/.test(typeName)) {
    return 'Mapping';
  } else if (/^t_array:dyn<.+>/.test(typeName)) {
    return 'DynArray';
  } else if (/^t_array:\d+<.+>/.test(typeName)) {
    return 'StaticArray';
  } else {
    return 'Elementary';
  }
}

type TypeKind = 'Elementary' | 'Mapping' | 'Struct' | 'Enum' | 'DynArray' | 'StaticArray';
type LegacyContracts = Record<string, ContractInterface>;
type LegacyProxies = Record<string, ProxyInterface[]>;
type AstIdGetter = (typeName: string) => number;
type LegacyTypes = Record<string, LegacyType>;
type NetworkExportData = Pick<NetworkFileData, 'proxies' | 'proxyFactory' | 'app' | 'package' | 'provider'>;

export interface MigrationOutput {
  newManifestsData: Record<string, ManifestData>;
  networksExportData: Record<string, NetworkExportData>;
}

interface LegacyType {
  id: string;
  kind: string;
  label: string;
  members?: TypeItemMembers;
}

interface LegacyStorageItem extends StorageItem {
  path: string;
  astId: number;
}

// All of the following types are taken from OpenZeppelin CLI verbatim

interface ContractInterface {
  address?: string;
  constructorCode?: string;
  localBytecodeHash?: string;
  deployedBytecodeHash?: string;
  bodyBytecodeHash?: string;
  /* eslint-disable */
  types?: any;
  storage?: any;
  warnings?: any;
  [id: string]: any;
  /* eslint-enable */
}

interface SolidityLibInterface {
  address: string;
  constructorCode: string;
  bodyBytecodeHash: string;
  localBytecodeHash: string;
  deployedBytecodeHash: string;
}

enum ProxyType {
  Upgradeable = 'Upgradeable',
  Minimal = 'Minimal',
  NonProxy = 'NonProxy',
}

interface ProxyInterface {
  contractName?: string;
  package?: string;
  address?: string;
  version?: string;
  implementation?: string;
  admin?: string;
  kind?: ProxyType;
  bytecodeHash?: string; // Only used for non-proxies from regulear deploys.
}

interface DependencyInterface {
  name?: string;
  package?: string;
  version?: string;
  customDeploy?: boolean;
}

interface AddressWrapper {
  address?: string;
}

export interface NetworkFileData {
  contracts: { [name: string]: ContractInterface };
  solidityLibs: { [name: string]: SolidityLibInterface };
  proxies: { [contractName: string]: ProxyInterface[] };
  manifestVersion?: string;
  zosversion?: string;
  proxyAdmin: AddressWrapper;
  proxyFactory: AddressWrapper;
  app: AddressWrapper;
  package: AddressWrapper;
  provider: AddressWrapper;
  version: string;
  frozen: boolean;
  dependencies: { [dependencyName: string]: DependencyInterface };
}

interface ConfigFileCompilerOptions {
  manager: string;
  solcVersion: string;
  contractsDir: string;
  artifactsDir: string;
  compilerSettings: {
    evmVersion: string;
    optimizer: {
      enabled: boolean;
      runs?: string;
    };
  };
  typechain: {
    enabled: boolean;
    outDir?: string;
    target?: string;
  };
}

export interface ProjectFileData {
  name: string;
  version: string;
  manifestVersion?: string;
  zosversion?: string;
  dependencies: { [name: string]: string };
  contracts: string[];
  publish: boolean;
  compiler: Partial<ConfigFileCompilerOptions>;
  telemetryOptIn?: boolean;
}
