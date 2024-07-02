import _test, { TestFn } from 'ava';
import { ContractDefinition } from 'solidity-ast';
import { ASTDereferencer, astDereferencer, findAll } from 'solidity-ast/utils';
import { artifacts } from 'hardhat';
import { extractStorageLayout } from './extract';
import { StorageLayoutComparator, stripContractSubstrings } from './compare';
import { StorageLayout, getDetailedLayout } from './layout';
import { getStorageUpgradeErrors } from '.';

interface Context {
  extractStorageLayout: (contract: string) => ReturnType<typeof extractStorageLayout>;
}

const test = _test as TestFn<Context>;

const dummyDecodeSrc = () => 'file.sol:1';
const testContracts = [
  'contracts/test/RetypeFromContract.sol:RetypeContractToUint160V1',
  'contracts/test/RetypeFromContract.sol:RetypeContractToUint160V2',
  'contracts/test/RetypeFromContract.sol:RetypeUint160ToContractV1',
  'contracts/test/RetypeFromContract.sol:RetypeUint160ToContractV2',
  'contracts/test/RetypeFromContract.sol:RetypeContractToUint160MappingV1',
  'contracts/test/RetypeFromContract.sol:RetypeContractToUint160MappingV2',
  'contracts/test/RetypeFromContract.sol:RetypeUint160ToContractMappingV1',
  'contracts/test/RetypeFromContract.sol:RetypeUint160ToContractMappingV2',
  'contracts/test/RetypeFromContract.sol:ImplicitRetypeV1',
  'contracts/test/RetypeFromContract.sol:ImplicitRetypeV2',
  'contracts/test/RetypeFromContract.sol:ImplicitRetypeMappingV1',
  'contracts/test/RetypeFromContract.sol:ImplicitRetypeMappingV2',
  'contracts/test/RetypeFromContract.sol:RetypeStructV1',
  'contracts/test/RetypeFromContract.sol:RetypeStructV2',
  'contracts/test/RetypeFromContract.sol:RetypeStructV2Bad',
  'contracts/test/RetypeFromContract.sol:RetypeEnumV1',
  'contracts/test/RetypeFromContract.sol:RetypeEnumV2',
  'contracts/test/RetypeFromContract.sol:RetypeEnumV2Bad',
];

test.before(async t => {
  const contracts: Record<string, ContractDefinition> = {};
  const deref: Record<string, ASTDereferencer> = {};
  const storageLayout: Record<string, StorageLayout> = {};
  for (const contract of testContracts) {
    const buildInfo = await artifacts.getBuildInfo(contract);
    if (buildInfo === undefined) {
      throw new Error(`Build info not found for contract ${contract}`);
    }
    const solcOutput = buildInfo.output;
    for (const def of findAll('ContractDefinition', solcOutput.sources['contracts/test/RetypeFromContract.sol'].ast)) {
      contracts[def.name] = def;
      deref[def.name] = astDereferencer(solcOutput);
      storageLayout[def.name] = (
        solcOutput.contracts['contracts/test/RetypeFromContract.sol'][def.name] as any
      ).storageLayout;
    }
  }

  t.context.extractStorageLayout = name =>
    extractStorageLayout(contracts[name], dummyDecodeSrc, deref[name], storageLayout[name]);
});

function getReport(original: StorageLayout, updated: StorageLayout) {
  const originalDetailed = getDetailedLayout(original);
  const updatedDetailed = getDetailedLayout(updated);
  const comparator = new StorageLayoutComparator();
  return comparator.compareLayouts(originalDetailed, updatedDetailed);
}

test('retype contract to uint160', t => {
  const v1 = t.context.extractStorageLayout('RetypeContractToUint160V1');
  const v2 = t.context.extractStorageLayout('RetypeContractToUint160V2');
  const report = getReport(v1, v2);
  t.true(report.ok, report.explain());
});

test('retype uint160 to contract', t => {
  const v1 = t.context.extractStorageLayout('RetypeUint160ToContractV1');
  const v2 = t.context.extractStorageLayout('RetypeUint160ToContractV2');
  const report = getReport(v1, v2);
  t.true(report.ok, report.explain());
});

test('retype contract to uint160 mapping', t => {
  const v1 = t.context.extractStorageLayout('RetypeContractToUint160MappingV1');
  const v2 = t.context.extractStorageLayout('RetypeContractToUint160MappingV2');
  const report = getReport(v1, v2);
  t.true(report.ok, report.explain());
});

test('retype uint160 to contract mapping', t => {
  const v1 = t.context.extractStorageLayout('RetypeUint160ToContractMappingV1');
  const v2 = t.context.extractStorageLayout('RetypeUint160ToContractMappingV2');
  const report = getReport(v1, v2);
  t.true(report.ok, report.explain());
});

test('implicit retype', t => {
  const v1 = t.context.extractStorageLayout('ImplicitRetypeV1');
  const v2 = t.context.extractStorageLayout('ImplicitRetypeV2');
  const report = getReport(v1, v2);
  t.true(report.ok, report.explain());
});

test('implicit retype mapping', t => {
  const v1 = t.context.extractStorageLayout('ImplicitRetypeMappingV1');
  const v2 = t.context.extractStorageLayout('ImplicitRetypeMappingV2');
  const report = getReport(v1, v2);
  t.true(report.ok, report.explain());
});

test('strip contract substrings', t => {
  t.is(stripContractSubstrings(undefined), undefined);
  t.is(stripContractSubstrings('address'), 'address');
  t.is(stripContractSubstrings('CustomContract'), 'CustomContract');
  t.is(stripContractSubstrings('contract CustomContract'), 'CustomContract');
  t.is(stripContractSubstrings('mapping(uint8 => CustomContract)'), 'mapping(uint8 => CustomContract)');
  t.is(stripContractSubstrings('mapping(uint8 => contract CustomContract)'), 'mapping(uint8 => CustomContract)');
  t.is(stripContractSubstrings('mapping(contract CustomContract => uint8)'), 'mapping(CustomContract => uint8)');
  t.is(stripContractSubstrings('mapping(contract A => contract B)'), 'mapping(A => B)');
  t.is(stripContractSubstrings('mapping(Substringcontract => address)'), 'mapping(Substringcontract => address)');
  t.is(
    stripContractSubstrings('mapping(contract Substringcontract => address)'),
    'mapping(Substringcontract => address)',
  );
  t.is(stripContractSubstrings('Mystruct'), 'Mystruct');
  t.is(stripContractSubstrings('struct Mystruct'), 'Mystruct');
  t.is(stripContractSubstrings('Myenum'), 'Myenum');
  t.is(stripContractSubstrings('enum Myenum'), 'Myenum');
});

test('retype from struct', t => {
  const v1 = t.context.extractStorageLayout('RetypeStructV1');
  const v2 = t.context.extractStorageLayout('RetypeStructV2');
  const report = getReport(v1, v2);
  t.true(report.ok, report.explain());
});

test('bad retype from struct', t => {
  const v1 = t.context.extractStorageLayout('RetypeStructV1');
  const v2 = t.context.extractStorageLayout('RetypeStructV2Bad');
  t.like(getStorageUpgradeErrors(v1, v2), {
    length: 1,
    0: {
      kind: 'layoutchange',
      original: { label: 'x' },
      updated: { label: 'x' },
    },
  });
});

test('retype from enum', t => {
  const v1 = t.context.extractStorageLayout('RetypeEnumV1');
  const v2 = t.context.extractStorageLayout('RetypeEnumV2');
  const report = getReport(v1, v2);
  t.true(report.ok, report.explain());
});

test('bad retype from enum', t => {
  const v1 = t.context.extractStorageLayout('RetypeEnumV1');
  const v2 = t.context.extractStorageLayout('RetypeEnumV2Bad');
  t.like(getStorageUpgradeErrors(v1, v2), {
    length: 1,
    0: {
      kind: 'layoutchange',
      original: { label: 'x' },
      updated: { label: 'x' },
    },
  });
});
