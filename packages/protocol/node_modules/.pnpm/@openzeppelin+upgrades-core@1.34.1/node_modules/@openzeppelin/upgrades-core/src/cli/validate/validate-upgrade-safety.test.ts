import test from 'ava';

import { promises as fs } from 'fs';
import { rimraf } from 'rimraf';
import path from 'path';
import os from 'os';

import { artifacts } from 'hardhat';
import { findSpecifiedContracts, validateUpgradeSafety, withCliDefaults } from './validate-upgrade-safety';
import { ReferenceContractNotFound } from './find-contract';

test.before(async () => {
  process.chdir(await fs.mkdtemp(path.join(os.tmpdir(), 'upgrades-core-test-')));
});

test.after(async () => {
  await rimraf(process.cwd());
});

test('validate upgrade safety', async t => {
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Validate.sol:Safe`);

  await fs.mkdir('validate-upgrade-safety');
  await fs.writeFile('validate-upgrade-safety/1.json', JSON.stringify(buildInfo));

  const report = await validateUpgradeSafety('validate-upgrade-safety');
  t.false(report.ok);
  t.snapshot(report.explain());
});

test('ambiguous upgrades-from', async t => {
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Validate.sol:Safe`);

  await fs.mkdir('ambiguous-upgrades-from');
  await fs.writeFile('ambiguous-upgrades-from/1.json', JSON.stringify(buildInfo));
  await fs.writeFile('ambiguous-upgrades-from/2.json', JSON.stringify(buildInfo));

  const error = await t.throwsAsync(validateUpgradeSafety('ambiguous-upgrades-from'));
  t.true(error?.message.includes('Found multiple contracts with name'), error?.message);
});

test('bad upgrade from 0.8.8 to 0.8.9', async t => {
  await fs.mkdir('bad-upgrade');

  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Storage088.sol:Storage088`);
  await fs.writeFile('bad-upgrade/storage088.json', JSON.stringify(buildInfo));

  const buildInfo2 = await artifacts.getBuildInfo(`contracts/test/cli/Storage089.sol:Storage089`);
  await fs.writeFile('bad-upgrade/storage089.json', JSON.stringify(buildInfo2));

  const report = await validateUpgradeSafety('bad-upgrade');
  t.false(report.ok);
  t.snapshot(report.explain());
});

test('reference contract not found', async t => {
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Storage089.sol:Storage089`);
  await fs.mkdir('ref-not-found');
  await fs.writeFile('ref-not-found/storage089.json', JSON.stringify(buildInfo));

  const error = await t.throwsAsync(validateUpgradeSafety('ref-not-found'));
  t.true(error instanceof ReferenceContractNotFound);
  t.is(
    error?.message,
    'Could not find contract Storage088 referenced in contracts/test/cli/Storage089.sol:Storage089.',
  );
});

test('invalid annotation args - upgrades-from', async t => {
  const fullyQualifiedName = `contracts/test/cli/InvalidAnnotationArgsUpgradesFrom.sol:InvalidAnnotationArgsUpgradesFrom`;

  const buildInfo = await artifacts.getBuildInfo(fullyQualifiedName);
  await fs.mkdir('invalid-annotation-args-upgrades-from');
  await fs.writeFile('invalid-annotation-args-upgrades-from/1.json', JSON.stringify(buildInfo));

  const error = await t.throwsAsync(validateUpgradeSafety('invalid-annotation-args-upgrades-from'));
  t.true(
    error?.message.includes(
      `Invalid number of arguments for @custom:oz-upgrades-from annotation in contract ${fullyQualifiedName}.`,
    ),
  );
  t.true(error?.message.includes('Found 0, expected 1'));
});

test('invalid annotation args - upgrades', async t => {
  const fullyQualifiedName = `contracts/test/cli/InvalidAnnotationArgsUpgrades.sol:InvalidAnnotationArgsUpgrades`;

  const buildInfo = await artifacts.getBuildInfo(fullyQualifiedName);
  await fs.mkdir('invalid-annotation-args-upgrades');
  await fs.writeFile('invalid-annotation-args-upgrades/1.json', JSON.stringify(buildInfo));

  const error = await t.throwsAsync(validateUpgradeSafety('invalid-annotation-args-upgrades'));
  t.true(
    error?.message.includes(
      `Invalid number of arguments for @custom:oz-upgrades annotation in contract ${fullyQualifiedName}.`,
    ),
  );
  t.true(error?.message.includes('Found 1, expected 0'));
});

test('findSpecifiedContracts - requireReference option without contract', async t => {
  try {
    findSpecifiedContracts([], withCliDefaults({ requireReference: true }));
  } catch (e: any) {
    t.true(
      e.message.includes(
        'The requireReference option can only be specified when the contract option is also specified.',
      ),
    );
  }
});
