import _test, { TestFn } from 'ava';
import { ContractDefinition } from 'solidity-ast';
import { findAll, astDereferencer } from 'solidity-ast/utils';
import { artifacts } from 'hardhat';

import { SolcOutput } from '../solc-api';
import { extractStorageLayout } from '../storage/extract';
import { StorageLayoutComparator } from './compare';
import { StorageLayout, getDetailedLayout } from './layout';

interface Context {
  extractStorageLayout: (contract: string) => ReturnType<typeof extractStorageLayout>;
}

const test = _test as TestFn<Context>;

const dummyDecodeSrc = () => 'file.sol:1';

test.before(async t => {
  const buildInfo = await artifacts.getBuildInfo('contracts/test/Storage.sol:Storage1');
  if (buildInfo === undefined) {
    throw new Error('Build info not found');
  }
  const solcOutput: SolcOutput = buildInfo.output;
  const contracts: Record<string, ContractDefinition> = {};
  for (const def of findAll('ContractDefinition', solcOutput.sources['contracts/test/Storage.sol'].ast)) {
    contracts[def.name] = def;
  }
  const deref = astDereferencer(solcOutput);
  t.context.extractStorageLayout = name => extractStorageLayout(contracts[name], dummyDecodeSrc, deref);
});

function getReport(original: StorageLayout, updated: StorageLayout) {
  const originalDetailed = getDetailedLayout(original);
  const updatedDetailed = getDetailedLayout(updated);
  const comparator = new StorageLayoutComparator();
  return comparator.compareLayouts(originalDetailed, updatedDetailed);
}

test('structs', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Struct_V1');
  const v2 = t.context.extractStorageLayout('StorageUpgrade_Struct_V2_Bad');
  const report = getReport(v1, v2);
  t.snapshot(report.explain());
});

test('enums', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Enum_V1');
  const v2 = t.context.extractStorageLayout('StorageUpgrade_Enum_V2_Bad');
  const report = getReport(v1, v2);
  t.snapshot(report.explain());
});

test('obvious mismatch', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_ObviousMismatch_V1');
  const v2 = t.context.extractStorageLayout('StorageUpgrade_ObviousMismatch_V2_Bad');
  const report = getReport(v1, v2);
  t.snapshot(report.explain());
});

test('function visibility change', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_FunctionType_Visibility_V1');
  const v2 = t.context.extractStorageLayout('StorageUpgrade_FunctionType_Visibility_V2_Bad');
  const report = getReport(v1, v2);
  t.snapshot(report.explain());
});

test('array', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Array_V1');
  const v2 = t.context.extractStorageLayout('StorageUpgrade_Array_V2_Bad');
  const report = getReport(v1, v2);
  t.snapshot(report.explain());
});

test('mapping', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Mapping_V1');
  const v2 = t.context.extractStorageLayout('StorageUpgrade_Mapping_V2_Bad');
  const report = getReport(v1, v2);
  t.snapshot(report.explain());
});

test('mapping enum key', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_MappingEnumKey_V1');
  const v2 = t.context.extractStorageLayout('StorageUpgrade_MappingEnumKey_V2_Bad');
  const report = getReport(v1, v2);
  t.snapshot(report.explain());
});

test('rename', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Rename_V1');
  const v2 = t.context.extractStorageLayout('StorageUpgrade_Rename_V2');
  const report = getReport(v1, v2);
  t.snapshot(report.explain());
});

test('replace', t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Replace_V1');
  const v2 = t.context.extractStorageLayout('StorageUpgrade_Replace_V2');
  const report = getReport(v1, v2);
  t.snapshot(report.explain());
});
