import { getProjectReport } from './project-report';
import { UpgradeableContractErrorReport } from '../../validate';

import _test, { TestFn } from 'ava';
import { ContractDefinition } from 'solidity-ast';
import { findAll, astDereferencer } from 'solidity-ast/utils';
import { artifacts } from 'hardhat';

import { SolcOutput } from '../../solc-api';
import { extractStorageLayout } from '../../storage/extract';
import { StorageLayoutComparator } from '../../storage/compare';
import { StorageLayout, getDetailedLayout } from '../../storage/layout';
import { UpgradeableContractReport } from './contract-report';

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

function getLayoutReport(original: StorageLayout, updated: StorageLayout) {
  const originalDetailed = getDetailedLayout(original);
  const updatedDetailed = getDetailedLayout(updated);
  const comparator = new StorageLayoutComparator();
  return comparator.compareLayouts(originalDetailed, updatedDetailed);
}

test('get project report - ok - no upgradeable', async t => {
  const report = getProjectReport([]);
  t.true(report.ok);
  t.is(report.numPassed, 0);
  t.is(report.numTotal, 0);
  t.is(report.explain(), 'No upgradeable contracts detected.');
});

test('get project report - ok - console', async t => {
  const report = getProjectReport([
    new UpgradeableContractReport(
      'mypath/MyContract.sol:MyContract1',
      undefined,
      new UpgradeableContractErrorReport([]),
      undefined,
    ),
  ]);

  t.true(report.ok);
  t.is(report.numPassed, 1);
  t.is(report.numTotal, 1);
  t.regex(report.explain(), /SUCCESS \(1 upgradeable contract detected, 1 passed, 0 failed\)/);
});

test('get project report - errors - console', async t => {
  const v1 = t.context.extractStorageLayout('StorageUpgrade_Replace_V1');
  const v2 = t.context.extractStorageLayout('StorageUpgrade_Replace_V2');
  const layoutReport = getLayoutReport(v1, v2);

  const report = getProjectReport([
    new UpgradeableContractReport(
      'mypath/MyContract.sol:MyContract',
      undefined,
      new UpgradeableContractErrorReport([
        {
          src: 'MyContract.sol:10',
          kind: 'missing-public-upgradeto',
        },
        {
          src: 'MyContract.sol:20',
          kind: 'delegatecall',
        },
      ]),
      undefined,
    ),
    new UpgradeableContractReport('MyContract2', 'MyContract', new UpgradeableContractErrorReport([]), layoutReport),
  ]);

  t.false(report.ok);
  t.is(report.numPassed, 0);
  t.is(report.numTotal, 2);
  t.snapshot(report.explain());
});

test('get project report - some passed', async t => {
  const report = getProjectReport([
    new UpgradeableContractReport(
      'mypath/MyContract.sol:MyContract1',
      undefined,
      new UpgradeableContractErrorReport([
        {
          src: 'MyContract.sol:10',
          kind: 'missing-public-upgradeto',
        },
      ]),
      undefined,
    ),
    new UpgradeableContractReport(
      'mypath/MyContract.sol:MyContract2',
      undefined,
      new UpgradeableContractErrorReport([]),
      undefined,
    ),
  ]);

  t.false(report.ok);
  t.is(report.numPassed, 1);
  t.is(report.numTotal, 2);
  t.snapshot(report.explain());
});
