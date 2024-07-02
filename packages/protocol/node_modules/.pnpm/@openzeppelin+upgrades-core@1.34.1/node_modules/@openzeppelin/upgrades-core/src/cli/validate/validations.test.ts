import _test, { ExecutionContext, TestFn } from 'ava';

import { artifacts } from 'hardhat';
import { validateBuildInfoContracts } from './validations';
import { UpgradeableContractReport, getContractReports } from './contract-report';
import { withCliDefaults } from './validate-upgrade-safety';

interface Context {
  reports: UpgradeableContractReport[];
}

const test = _test as TestFn<Context>;

const SOURCE_FILE = 'contracts/test/cli/Validate.sol';

test.before(async t => {
  const buildInfo = await artifacts.getBuildInfo(`${SOURCE_FILE}:Safe`);

  if (buildInfo === undefined) {
    t.fail();
  } else {
    const sourceContracts = validateBuildInfoContracts([buildInfo]);
    t.context.reports = getContractReports(sourceContracts, withCliDefaults({}));
  }
});

function getReport(t: ExecutionContext<Context>, contractName: string) {
  return t.context.reports.find(r => r.contract === `${SOURCE_FILE}:${contractName}`);
}

function assertReport(
  t: ExecutionContext<Context>,
  report: UpgradeableContractReport | undefined,
  valid: boolean | undefined,
) {
  if (valid === undefined) {
    t.true(report === undefined);
  } else if (valid === true) {
    t.true(report !== undefined);
    t.true(report?.ok);
    // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
    t.regex(report!.explain(), /✔/);
  } else if (valid === false) {
    t.true(report !== undefined);
    t.false(report?.ok);
    t.snapshot(report?.explain());
  }
}

function testValid(name: string, valid: boolean | undefined) {
  const expectationString = valid === undefined ? 'ignores' : valid ? 'accepts' : 'rejects';
  const testName = [expectationString, name].join(' ');
  test(testName, t => {
    const report = getReport(t, name);
    assertReport(t, report, valid);
  });
}

testValid('Safe', true);
testValid('MultipleUnsafe', false);
testValid('NonUpgradeable', undefined);
testValid('HasInitializer', true);
testValid('HasUpgradeTo', true);
testValid('HasUpgradeToConstructorUnsafe', false);
testValid('InheritsMultipleUnsafe', false);
testValid('UpgradesFromUUPS', false);
testValid('UpgradesFromTransparent', true);
testValid('UnsafeAndStorageLayoutErrors', false);
testValid('BecomesSafe', true);
testValid('BecomesBadLayout', false);
testValid('StillUnsafe', false);
testValid('AbstractUpgradeable', undefined);
testValid('InheritsAbstractUpgradeable', true);
