import _test, { TestFn } from 'ava';
import { artifacts } from 'hardhat';

import SOLIDITY_VERSION from './solidity-version.json';

import { SolcInput, SolcOutput } from './solc-api';
import { UpgradeableContract } from './standalone';

interface Context {
  solcInput: SolcInput;
  solcOutput: SolcOutput;
}

const test = _test as TestFn<Context>;

test.before(async t => {
  const buildInfo = await artifacts.getBuildInfo('contracts/test/Standalone.sol:StandaloneV1');
  if (buildInfo === undefined) {
    throw new Error('Build info not found');
  }
  t.context.solcInput = buildInfo.input;
  t.context.solcOutput = buildInfo.output;
});

test('reports unsafe operation', t => {
  const impl = new UpgradeableContract('StandaloneV1', t.context.solcInput, t.context.solcOutput, {}, SOLIDITY_VERSION);
  const report = impl.getErrorReport();
  t.false(report.ok);
  t.true(report.errors[0].kind === 'delegatecall');
});

test('reports unsafe operation - fully qualified name', t => {
  const impl = new UpgradeableContract(
    'contracts/test/Standalone.sol:StandaloneV1',
    t.context.solcInput,
    t.context.solcOutput,
    {},
    SOLIDITY_VERSION,
  );
  const report = impl.getErrorReport();
  t.false(report.ok);
  t.true(report.errors[0].kind === 'delegatecall');
});

test('reports storage upgrade errors', t => {
  const v1 = new UpgradeableContract('StandaloneV1', t.context.solcInput, t.context.solcOutput, {}, SOLIDITY_VERSION);

  const v2Good = new UpgradeableContract(
    'StandaloneV2Good',
    t.context.solcInput,
    t.context.solcOutput,
    {},
    SOLIDITY_VERSION,
  );
  const goodReport = v1.getStorageUpgradeReport(v2Good);
  t.true(goodReport.ok);

  const v2Bad = new UpgradeableContract(
    'StandaloneV2Bad',
    t.context.solcInput,
    t.context.solcOutput,
    {},
    SOLIDITY_VERSION,
  );
  const badReport = v1.getStorageUpgradeReport(v2Bad);
  t.false(badReport.ok);
});

test('dont report renamed version update', t => {
  const v1 = new UpgradeableContract(
    'StandaloneRenameV1',
    t.context.solcInput,
    t.context.solcOutput,
    {},
    SOLIDITY_VERSION,
  );

  const v2 = new UpgradeableContract(
    'StandaloneRenameV2',
    t.context.solcInput,
    t.context.solcOutput,
    {},
    SOLIDITY_VERSION,
  );
  const goodReport = v1.getStorageUpgradeReport(v2);
  t.true(goodReport.ok);

  const v3 = new UpgradeableContract(
    'StandaloneRenameV3',
    t.context.solcInput,
    t.context.solcOutput,
    {},
    SOLIDITY_VERSION,
  );
  const goodReport2 = v2.getStorageUpgradeReport(v3);
  t.true(goodReport2.ok);
});

test('namespaced output', async t => {
  const buildInfo = await artifacts.getBuildInfo('contracts/test/Namespaced.sol:Example');
  if (buildInfo === undefined) {
    throw new Error('Build info not found');
  }

  const impl = new UpgradeableContract('Example', buildInfo.input, buildInfo.output, {}, '0.8.20');
  const report = impl.getErrorReport();
  t.true(report.ok);
});

test('namespaced output without version', async t => {
  const buildInfo = await artifacts.getBuildInfo('contracts/test/Namespaced.sol:Example');
  if (buildInfo === undefined) {
    throw new Error('Build info not found');
  }

  const error = t.throws(() => new UpgradeableContract('Example', buildInfo.input, buildInfo.output));
  t.assert(
    error?.message.includes(
      `contracts/test/Namespaced.sol: Namespace annotations require Solidity version >= 0.8.20, but no solcVersion parameter was provided`,
    ),
    error?.message,
  );
});
