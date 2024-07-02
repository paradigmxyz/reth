import _test, { TestFn } from 'ava';
import { ContractDefinition } from 'solidity-ast';
import { findAll, astDereferencer } from 'solidity-ast/utils';
import { artifacts } from 'hardhat';

import { SolcOutput } from './solc-api';
import { getStorageUpgradeErrors } from './storage';
import { StorageLayout } from './storage/layout';
import { extractStorageLayout } from './storage/extract';
import { solcInputOutputDecoder } from './src-decoder';

interface Context {
  extractStorageLayout: (contract: string, layoutInfo: boolean) => ReturnType<typeof extractStorageLayout>;
}

const test = _test as TestFn<Context>;

test.before(async t => {
  const origBuildInfo = await artifacts.getBuildInfo('contracts/test/NamespacedUDVT.sol:NamespacedUDVT');
  const namespacedBuildInfo = await artifacts.getBuildInfo('contracts/test/NamespacedUDVTLayout.sol:NamespacedUDVT');

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
  for (const def of findAll('ContractDefinition', origSolcOutput.sources['contracts/test/NamespacedUDVT.sol'].ast)) {
    origContractDefs.push(def);
  }
  const namespacedContractDefs = [];
  for (const def of findAll(
    'ContractDefinition',
    namespacedSolcOutput.sources['contracts/test/NamespacedUDVTLayout.sol'].ast,
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
      origSolcOutput.contracts['contracts/test/NamespacedUDVT.sol'][origContractDef.name].storageLayout!;
    // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
    namespacedStorageLayouts[namespacedContractDef.name] =
      namespacedSolcOutput.contracts['contracts/test/NamespacedUDVTLayout.sol'][
        namespacedContractDef.name
      ].storageLayout!;
  }
  const origDeref = astDereferencer(origSolcOutput);
  const namespacedDeref = astDereferencer(namespacedSolcOutput);

  const decodeSrc = solcInputOutputDecoder(origBuildInfo.input, origSolcOutput);
  t.context.extractStorageLayout = (name, layoutInfo) =>
    extractStorageLayout(
      origContracts[name],
      decodeSrc,
      origDeref,
      origStorageLayouts[name],
      layoutInfo
        ? {
            deref: namespacedDeref,
            contractDef: namespacedContracts[name],
            storageLayout: namespacedStorageLayouts[name],
          }
        : undefined,
    );
});

test('user defined value types - layout info', async t => {
  const v1 = t.context.extractStorageLayout('NamespacedUDVT', true);
  const v2 = t.context.extractStorageLayout('NamespacedUDVT_V2_Ok', true);
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.deepEqual(comparison, []);
});

test('user defined value types - no layout info', async t => {
  const v1 = t.context.extractStorageLayout('NamespacedUDVT', false);
  const v2 = t.context.extractStorageLayout('NamespacedUDVT_V2_Ok', false);
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.deepEqual(comparison, []);
});

test('user defined value types - layout info - bad underlying type', async t => {
  const v1 = t.context.extractStorageLayout('NamespacedUDVT', true);
  const v2 = t.context.extractStorageLayout('NamespacedUDVT_V2_Resize', true);
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.like(comparison, {
    length: 1,
    0: {
      kind: 'typechange',
      change: {
        kind: 'type resize',
      },
      original: { label: 'my_user_value' },
      updated: { label: 'my_user_value' },
    },
  });
});

test('user defined value types - no layout info - bad underlying type', async t => {
  const v1 = t.context.extractStorageLayout('NamespacedUDVT', false);
  const v2 = t.context.extractStorageLayout('NamespacedUDVT_V2_Resize', false);
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.like(comparison, {
    length: 1,
    0: {
      kind: 'typechange',
      change: {
        kind: 'unknown',
      },
      original: { label: 'my_user_value' },
      updated: { label: 'my_user_value' },
    },
  });
});

test('mapping with user defined value type key - ok', t => {
  const v1 = t.context.extractStorageLayout('NamespacedUDVT_MappingKey_V1', true);
  const v2 = t.context.extractStorageLayout('NamespacedUDVT_MappingKey_V2_Ok', true);
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.deepEqual(comparison, []);
});

test('mapping with user defined value type key - bad', t => {
  const v1 = t.context.extractStorageLayout('NamespacedUDVT_MappingKey_V1', true);
  const v2 = t.context.extractStorageLayout('NamespacedUDVT_MappingKey_V2_Bad', true);
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.like(comparison, {
    length: 2,
    0: {
      kind: 'typechange',
      change: {
        kind: 'mapping key',
      },
      original: { label: 'm1' },
      updated: { label: 'm1' },
    },
    1: {
      kind: 'typechange',
      change: {
        kind: 'mapping key',
      },
      original: { label: 'm2' },
      updated: { label: 'm2' },
    },
  });
});
