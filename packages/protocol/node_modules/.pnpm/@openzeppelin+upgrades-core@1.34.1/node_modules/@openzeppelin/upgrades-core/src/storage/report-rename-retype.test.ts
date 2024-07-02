import _test, { TestFn } from 'ava';
import { ContractDefinition } from 'solidity-ast';
import { ASTDereferencer, astDereferencer, findAll } from 'solidity-ast/utils';
import { artifacts } from 'hardhat';
import { extractStorageLayout } from './extract';
import { StorageLayoutComparator } from './compare';
import { StorageLayout, getDetailedLayout } from './layout';
import { getStorageUpgradeErrors } from '.';

interface Context {
  extractStorageLayout: (contract: string) => ReturnType<typeof extractStorageLayout>;
}

const test = _test as TestFn<Context>;

const dummyDecodeSrc = () => 'file.sol:1';
const testContracts = [
  'contracts/test/RenamedRetyped.sol:RenameV1',
  'contracts/test/RenamedRetyped.sol:RenameV2',
  'contracts/test/RenamedRetyped.sol:RetypeV1',
  'contracts/test/RenamedRetyped.sol:RetypeV2',
  'contracts/test/RenamedRetyped.sol:WronglyReportedRetypeV3',
  'contracts/test/RenamedRetyped.sol:MissmatchingTypeRetypeV4',
  'contracts/test/RenamedRetyped.sol:ConfusingRetypeV1',
  'contracts/test/RenamedRetyped.sol:ConfusingRetypeV2',
  'contracts/test/RenamedRetyped.sol:NonHardcodedRetypeV1',
  'contracts/test/RenamedRetyped.sol:NonHardcodedRetypeV2',
  'contracts/test/RenamedRetyped.sol:LayoutChangeV1',
  'contracts/test/RenamedRetyped.sol:LayoutChangeV2',
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
    for (const def of findAll('ContractDefinition', solcOutput.sources['contracts/test/RenamedRetyped.sol'].ast)) {
      contracts[def.name] = def;
      deref[def.name] = astDereferencer(solcOutput);
      storageLayout[def.name] = (
        solcOutput.contracts['contracts/test/RenamedRetyped.sol'][def.name] as any
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

test('successful rename', t => {
  const v1 = t.context.extractStorageLayout('RenameV1');
  const v2 = t.context.extractStorageLayout('RenameV2');
  const report = getReport(v1, v2);
  t.true(report.ok);
  t.snapshot(report.explain());
});

test('successful retype', t => {
  const v1 = t.context.extractStorageLayout('RetypeV1');
  const v2 = t.context.extractStorageLayout('RetypeV2');
  const report = getReport(v1, v2);
  t.true(report.ok);
  t.snapshot(report.explain());
});

test('wrongly reported retype', t => {
  const v1 = t.context.extractStorageLayout('RetypeV1');
  const v2 = t.context.extractStorageLayout('WronglyReportedRetypeV3');
  const report = getReport(v1, v2);
  t.false(report.ok);
  t.snapshot(report.explain());
});

test('rightly reported retype but incompatible new type', t => {
  const v1 = t.context.extractStorageLayout('RetypeV1');
  const v2 = t.context.extractStorageLayout('MissmatchingTypeRetypeV4');
  const report = getReport(v1, v2);
  t.false(report.ok);
  t.snapshot(report.explain());
});

test('confusing bad retype', t => {
  const v1 = t.context.extractStorageLayout('ConfusingRetypeV1');
  const v2 = t.context.extractStorageLayout('ConfusingRetypeV2');
  const report = getReport(v1, v2);
  t.false(report.ok);
  t.snapshot(report.explain());
});

test('non-hardcoded retype', t => {
  const v1 = t.context.extractStorageLayout('NonHardcodedRetypeV1');
  const v2 = t.context.extractStorageLayout('NonHardcodedRetypeV2');
  const report = getReport(v1, v2);
  t.true(report.ok);
  t.snapshot(report.explain());
});

test('retype with layout change', t => {
  const v1 = t.context.extractStorageLayout('LayoutChangeV1');
  const v2 = t.context.extractStorageLayout('LayoutChangeV2');

  // ensure both variables' layout changes appear in the internal errors
  t.like(getStorageUpgradeErrors(v1, v2), {
    length: 2,
    0: {
      kind: 'layoutchange',
      original: { label: 'a' },
      updated: { label: 'a' },
    },
    1: {
      kind: 'layoutchange',
      original: { label: 'b' },
      updated: { label: 'b' },
    },
  });

  const report = getReport(v1, v2);
  t.false(report.ok);
  t.snapshot(report.explain());
});
