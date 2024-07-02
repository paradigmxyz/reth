import _test, { TestFn } from 'ava';
import { ContractDefinition } from 'solidity-ast';
import { findAll, astDereferencer } from 'solidity-ast/utils';
import { artifacts } from 'hardhat';

import { SolcOutput } from './solc-api';
import { getStorageUpgradeErrors } from './storage';
import { StorageLayout } from './storage/layout';
import { extractStorageLayout } from './storage/extract';
import { stabilizeStorageLayout } from './utils/stabilize-layout';

interface Context {
  extractStorageLayout: (contract: string) => ReturnType<typeof extractStorageLayout>;
}

const test = _test as TestFn<Context>;

test.before(async t => {
  const buildInfo = await artifacts.getBuildInfo('contracts/test/Namespaced.sol:Example');
  if (buildInfo === undefined) {
    throw new Error('Build info not found');
  }
  const solcOutput: SolcOutput = buildInfo.output;
  const contracts: Record<string, ContractDefinition> = {};
  const storageLayouts: Record<string, StorageLayout> = {};
  for (const def of findAll('ContractDefinition', solcOutput.sources['contracts/test/Namespaced.sol'].ast)) {
    contracts[def.name] = def;
    // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
    storageLayouts[def.name] = solcOutput.contracts['contracts/test/Namespaced.sol'][def.name].storageLayout!;
  }
  const deref = astDereferencer(solcOutput);
  t.context.extractStorageLayout = name =>
    extractStorageLayout(contracts[name], dummyDecodeSrc, deref, storageLayouts[name]);
});

const dummyDecodeSrc = () => 'file.sol:1';

test('layout', t => {
  const layout = t.context.extractStorageLayout('Example');
  t.snapshot(stabilizeStorageLayout(layout));
});

test('multiple namespaces', t => {
  const layout = t.context.extractStorageLayout('MultipleNamespaces');
  t.snapshot(stabilizeStorageLayout(layout));
});

test('namespaced upgrade ok', t => {
  const v1 = t.context.extractStorageLayout('Example');
  const v2 = t.context.extractStorageLayout('ExampleV2_Ok');
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.deepEqual(comparison, []);
});

test('namespaced upgrade bad', t => {
  const v1 = t.context.extractStorageLayout('Example');
  const v2 = t.context.extractStorageLayout('ExampleV2_Bad');
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.like(comparison, {
    length: 1,
    0: {
      kind: 'delete',
      original: {
        contract: 'Example',
        label: 'x',
        type: {
          id: 't_uint256',
        },
      },
    },
  });
});

test('recursive struct outer ok', t => {
  const v1 = t.context.extractStorageLayout('RecursiveStruct');
  const v2 = t.context.extractStorageLayout('RecursiveStructV2_Outer_Ok');
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.deepEqual(comparison, []);
});

test('recursive struct bad', t => {
  const v1 = t.context.extractStorageLayout('RecursiveStruct');
  const v2 = t.context.extractStorageLayout('RecursiveStructV2_Bad');
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.like(comparison, {
    length: 1,
    0: {
      kind: 'typechange',
      change: {
        kind: 'struct members',
        ops: {
          length: 1,
          0: { kind: 'append' },
        },
      },
      original: { label: 's' },
      updated: { label: 's' },
    },
  });
});

test('multiple namespaces and regular variables ok', t => {
  const v1 = t.context.extractStorageLayout('MultipleNamespacesAndRegularVariables');
  const v2 = t.context.extractStorageLayout('MultipleNamespacesAndRegularVariablesV2_Ok');
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.deepEqual(comparison, []);
});

test('multiple namespaces and regular variables bad', t => {
  const v1 = t.context.extractStorageLayout('MultipleNamespacesAndRegularVariables');
  const v2 = t.context.extractStorageLayout('MultipleNamespacesAndRegularVariablesV2_Bad');
  const comparison = getStorageUpgradeErrors(v1, v2);
  t.like(comparison, {
    length: 5,
    0: {
      kind: 'insert',
      updated: {
        label: 'c',
      },
    },
    1: {
      kind: 'layoutchange',
      updated: {
        label: 'a', // layout available for regular variable outside of namespace
      },
    },
    2: {
      kind: 'layoutchange',
      updated: {
        label: 'b', // layout available for regular variable outside of namespace
      },
    },
    3: {
      kind: 'insert',
      updated: {
        label: 'c',
      },
    },
    4: {
      kind: 'insert',
      updated: {
        label: 'c',
      },
    },
  });
});
