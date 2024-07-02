import _test, { TestFn } from 'ava';
import { ContractDefinition } from 'solidity-ast';
import { findAll, astDereferencer } from 'solidity-ast/utils';
import { artifacts } from 'hardhat';

import { SolcOutput } from './solc-api';
import { StorageLayout } from './storage/layout';
import { extractStorageLayout } from './storage/extract';
import { solcInputOutputDecoder } from './src-decoder';

interface Context {
  extractStorageLayout: (contract: string) => ReturnType<typeof extractStorageLayout>;
}

const test = _test as TestFn<Context>;

test.before(async t => {
  const buildInfo = await artifacts.getBuildInfo('contracts/test/NamespacedConflicts.sol:DuplicateNamespace');
  if (buildInfo === undefined) {
    throw new Error('Build info not found');
  }
  const solcOutput: SolcOutput = buildInfo.output;
  const contracts: Record<string, ContractDefinition> = {};
  const storageLayouts: Record<string, StorageLayout> = {};
  for (const def of findAll('ContractDefinition', solcOutput.sources['contracts/test/NamespacedConflicts.sol'].ast)) {
    contracts[def.name] = def;
    // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
    storageLayouts[def.name] = solcOutput.contracts['contracts/test/NamespacedConflicts.sol'][def.name].storageLayout!;
  }
  const deref = astDereferencer(solcOutput);
  const decodeSrc = solcInputOutputDecoder(buildInfo.input, solcOutput);
  t.context.extractStorageLayout = name =>
    extractStorageLayout(contracts[name], decodeSrc, deref, storageLayouts[name]);
});

test('duplicate namespace', t => {
  const error = t.throws(() => t.context.extractStorageLayout('DuplicateNamespace'));
  t.snapshot(error?.message);
});

test('inherits duplicate', t => {
  const error = t.throws(() => t.context.extractStorageLayout('InheritsDuplicate'));
  t.snapshot(error?.message);
});

test('conflicts with parent', t => {
  const error = t.throws(() => t.context.extractStorageLayout('ConflictsWithParent'));
  t.snapshot(error?.message);
});

test('conflicts in both parents', t => {
  const error = t.throws(() => t.context.extractStorageLayout('ConflictsInBothParents'));
  t.snapshot(error?.message);
});
