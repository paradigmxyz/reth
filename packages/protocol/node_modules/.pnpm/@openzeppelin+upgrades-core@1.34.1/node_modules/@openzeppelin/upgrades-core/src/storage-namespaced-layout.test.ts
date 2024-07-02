import _test, { TestFn } from 'ava';
import { ContractDefinition } from 'solidity-ast';
import { findAll, astDereferencer } from 'solidity-ast/utils';
import { artifacts } from 'hardhat';

import { SolcOutput } from './solc-api';
import { StorageLayout } from './storage/layout';
import { extractStorageLayout } from './storage/extract';
import { stabilizeStorageLayout } from './utils/stabilize-layout';
import { solcInputOutputDecoder } from './src-decoder';

interface Context {
  extractStorageLayout: (contract: string) => ReturnType<typeof extractStorageLayout>;
}

const test = _test as TestFn<Context>;

test.before(async t => {
  // Tests extracting the storage layout (to include slot and offset) using Namespaced.sol's Example as the original contract,
  // and NamespacedLayout.sol's Example as the modified contract with the storage layout.
  const origBuildInfo = await artifacts.getBuildInfo('contracts/test/Namespaced.sol:Example');
  const namespacedBuildInfo = await artifacts.getBuildInfo('contracts/test/NamespacedLayout.sol:Example');

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
  for (const def of findAll('ContractDefinition', origSolcOutput.sources['contracts/test/Namespaced.sol'].ast)) {
    origContractDefs.push(def);
  }
  const namespacedContractDefs = [];
  for (const def of findAll(
    'ContractDefinition',
    namespacedSolcOutput.sources['contracts/test/NamespacedLayout.sol'].ast,
  )) {
    namespacedContractDefs.push(def);
  }

  // Expects the first contract in Namespaced.sol and NamespacedLayout.sol to be 'Example'
  const origContractDef = origContractDefs[0];
  const namespacedContractDef = namespacedContractDefs[0];

  origContracts[origContractDef.name] = origContractDef;
  namespacedContracts[namespacedContractDef.name] = namespacedContractDef;

  // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
  origStorageLayouts[origContractDef.name] =
    origSolcOutput.contracts['contracts/test/Namespaced.sol'][origContractDef.name].storageLayout!;
  // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
  namespacedStorageLayouts[namespacedContractDef.name] =
    namespacedSolcOutput.contracts['contracts/test/NamespacedLayout.sol'][namespacedContractDef.name].storageLayout!;

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

test('layout', t => {
  const layout = t.context.extractStorageLayout('Example');
  t.snapshot(stabilizeStorageLayout(layout));
});
