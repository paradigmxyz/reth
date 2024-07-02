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
  const origBuildInfo = await artifacts.getBuildInfo('contracts/test/NamespacedConflicts.sol:DuplicateNamespace');
  const namespacedBuildInfo = await artifacts.getBuildInfo(
    'contracts/test/NamespacedConflictsLayout.sol:DuplicateNamespace',
  );

  if (origBuildInfo === undefined || namespacedBuildInfo === undefined) {
    throw new Error('Build info not found');
  }

  const origSolcOutput: SolcOutput = origBuildInfo.output;
  const origContracts: Record<string, ContractDefinition> = {};
  const origStorageLayouts: Record<string, StorageLayout> = {};

  const namespacedSolcOutput: SolcOutput = namespacedBuildInfo.output;
  const namespacedContracts: Record<string, ContractDefinition> = {};
  const namespacedStorageLayouts: Record<string, StorageLayout> = {};

  const origContractDefs = [];
  for (const def of findAll(
    'ContractDefinition',
    origSolcOutput.sources['contracts/test/NamespacedConflicts.sol'].ast,
  )) {
    origContractDefs.push(def);
  }
  const namespacedContractDefs = [];
  for (const def of findAll(
    'ContractDefinition',
    namespacedSolcOutput.sources['contracts/test/NamespacedConflictsLayout.sol'].ast,
  )) {
    namespacedContractDefs.push(def);
  }

  for (let i = 0; i < origContractDefs.length; i++) {
    const origContractDef = origContractDefs[i];
    const namespacedContractDef = namespacedContractDefs[i];

    origContracts[origContractDef.name] = origContractDef;
    namespacedContracts[namespacedContractDef.name] = namespacedContractDef;

    // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
    origStorageLayouts[origContractDef.name] =
      origSolcOutput.contracts['contracts/test/NamespacedConflicts.sol'][origContractDef.name].storageLayout!;
    // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
    namespacedStorageLayouts[namespacedContractDef.name] =
      namespacedSolcOutput.contracts['contracts/test/NamespacedConflictsLayout.sol'][
        namespacedContractDef.name
      ].storageLayout!;
  }
  const origDeref = astDereferencer(origSolcOutput);
  const namespacedDeref = astDereferencer(namespacedSolcOutput);

  const decodeSrc = solcInputOutputDecoder(origBuildInfo.input, origSolcOutput);
  t.context.extractStorageLayout = name =>
    extractStorageLayout(origContracts[name], decodeSrc, origDeref, origStorageLayouts[name], {
      deref: namespacedDeref,
      contractDef: namespacedContracts[name],
      storageLayout: namespacedStorageLayouts[name],
    });
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
