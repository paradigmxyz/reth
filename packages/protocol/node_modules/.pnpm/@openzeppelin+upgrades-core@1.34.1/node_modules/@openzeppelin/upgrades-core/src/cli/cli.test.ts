import test, { ExecutionContext } from 'ava';
import { promisify } from 'util';
import { exec } from 'child_process';
import { promises as fs } from 'fs';
import path from 'path';
import os from 'os';
import { rimraf } from 'rimraf';
import { artifacts } from 'hardhat';

const execAsync = promisify(exec);

const CLI = 'node dist/cli/cli.js';

async function getTempDir(t: ExecutionContext) {
  const temp = await fs.mkdtemp(path.join(os.tmpdir(), 'upgrades-core-test-'));
  t.teardown(() => rimraf(temp));
  return temp;
}

test('help', async t => {
  const output = (await execAsync(`${CLI} validate --help`)).stdout;
  t.snapshot(output);
});

test('no args', async t => {
  const output = (await execAsync(CLI)).stdout;
  t.snapshot(output);
});

test('validate - errors', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Validate.sol:Safe`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const error = await t.throwsAsync(execAsync(`${CLI} validate ${temp}`));
  const expectation: string[] = [`Stdout: ${(error as any).stdout}`, `Stderr: ${(error as any).stderr}`];
  t.snapshot(expectation.join('\n'));
});

test('validate - single contract', async t => {
  // This should check even though the contract is not detected as upgradeable, since the --contract option was used.
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Validate.sol:NonUpgradeable`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const error = await t.throwsAsync(execAsync(`${CLI} validate ${temp} --contract NonUpgradeable`));
  const expectation: string[] = [`Stdout: ${(error as any).stdout}`, `Stderr: ${(error as any).stderr}`];
  t.snapshot(expectation.join('\n'));
});

test('validate - single contract, has upgrades-from', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Validate.sol:UnsafeAndStorageLayoutErrors`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const error = await t.throwsAsync(execAsync(`${CLI} validate ${temp} --contract UnsafeAndStorageLayoutErrors`));
  const expectation: string[] = [`Stdout: ${(error as any).stdout}`, `Stderr: ${(error as any).stderr}`];
  t.snapshot(expectation.join('\n'));
});

test('validate - single contract, reference overrides upgrades-from', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Validate.sol:UnsafeAndStorageLayoutErrors`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const error = await t.throwsAsync(
    execAsync(`${CLI} validate ${temp} --contract UnsafeAndStorageLayoutErrors --reference Safe`),
  );
  const expectation: string[] = [`Stdout: ${(error as any).stdout}`, `Stderr: ${(error as any).stderr}`];
  t.snapshot(expectation.join('\n'));
});

test('validate - single contract, reference is uups, overrides upgrades-from', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Validate.sol:UnsafeAndStorageLayoutErrors`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const error = await t.throwsAsync(
    execAsync(`${CLI} validate ${temp} --contract BecomesBadLayout --reference HasUpgradeTo`),
  );
  const expectation: string[] = [`Stdout: ${(error as any).stdout}`, `Stderr: ${(error as any).stderr}`];
  t.snapshot(expectation.join('\n'));
});

test('validate - single contract, reference', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Validate.sol:BecomesBadLayout`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const error = await t.throwsAsync(
    execAsync(`${CLI} validate ${temp} --contract BecomesBadLayout --reference StorageV1`),
  );
  const expectation: string[] = [`Stdout: ${(error as any).stdout}`, `Stderr: ${(error as any).stderr}`];
  t.snapshot(expectation.join('\n'));
});

test('validate - single contract, reference, fully qualified names', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Validate.sol:BecomesBadLayout`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const error = await t.throwsAsync(
    execAsync(
      `${CLI} validate ${temp} --contract contracts/test/cli/Validate.sol:BecomesBadLayout --reference contracts/test/cli/Validate.sol:StorageV1`,
    ),
  );
  const expectation: string[] = [`Stdout: ${(error as any).stdout}`, `Stderr: ${(error as any).stderr}`];
  t.snapshot(expectation.join('\n'));
});

test('validate - reference without contract option', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Validate.sol:StorageV1`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const error = await t.throwsAsync(execAsync(`${CLI} validate ${temp} --reference StorageV1`));
  t.true(
    error?.message.includes('The --reference option can only be used along with the --contract option.'),
    error?.message,
  );
});

test('validate - empty contract string', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Validate.sol:StorageV1`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const error = await t.throwsAsync(execAsync(`${CLI} validate ${temp} --contract --reference StorageV1`));
  t.true(error?.message.includes('Invalid option: --contract cannot be empty'), error?.message);
});

test('validate - blank contract string', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Validate.sol:StorageV1`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const error = await t.throwsAsync(execAsync(`${CLI} validate ${temp} --contract '    ' --reference StorageV1`));
  t.true(error?.message.includes('Invalid option: --contract cannot be empty'), error?.message);
});

test('validate - empty reference string', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Validate.sol:StorageV1`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const error = await t.throwsAsync(execAsync(`${CLI} validate ${temp} --contract StorageV1 --reference`));
  t.true(error?.message.includes('Invalid option: --reference cannot be empty'), error?.message);
});

test('validate - single contract not found', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Validate.sol:BecomesBadLayout`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const error = await t.throwsAsync(execAsync(`${CLI} validate ${temp} --contract NonExistent`));
  t.true(error?.message.includes('Could not find contract NonExistent.'), error?.message);
});

test('validate - reference not found', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Validate.sol:BecomesBadLayout`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const error = await t.throwsAsync(
    execAsync(`${CLI} validate ${temp} --contract BecomesBadLayout --reference NonExistent`),
  );
  t.true(error?.message.includes('Could not find contract NonExistent.'), error?.message);
});

test('validate - requireReference - no contract option', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Validate.sol:StorageV1`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const error = await t.throwsAsync(execAsync(`${CLI} validate ${temp} --requireReference`));
  t.true(
    error?.message.includes('The --requireReference option can only be used along with the --contract option.'),
    error?.message,
  );
});

test('validate - requireReference - no reference, no upgradesFrom', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Validate.sol:StorageV1`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const error = await t.throwsAsync(execAsync(`${CLI} validate ${temp} --contract StorageV1 --requireReference`));
  t.true(error?.message.includes('does not specify what contract it upgrades from'), error?.message);
});

test('validate - requireReference and unsafeSkipStorageCheck', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Validate.sol:StorageV1`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const error = await t.throwsAsync(
    execAsync(`${CLI} validate ${temp} --contract StorageV1 --requireReference --unsafeSkipStorageCheck`),
  );
  t.true(
    error?.message.includes('The requireReference and unsafeSkipStorageCheck options cannot be used at the same time.'),
    error?.message,
  );
});

test('validate - requireReference - no reference, has upgradesFrom - safe', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Validate.sol:BecomesSafe`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const output = (await execAsync(`${CLI} validate ${temp} --contract BecomesSafe --requireReference`)).stdout;
  t.snapshot(output);
});

test('validate - requireReference - no reference, has upgradesFrom - unsafe', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Validate.sol:BecomesBadLayout`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const error = await t.throwsAsync(
    execAsync(`${CLI} validate ${temp} --contract BecomesBadLayout --requireReference`),
  );
  const expectation: string[] = [`Stdout: ${(error as any).stdout}`, `Stderr: ${(error as any).stderr}`];
  t.snapshot(expectation.join('\n'));
});

test('validate - requireReference - has reference - unsafe', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Validate.sol:StorageV2_Bad_NoAnnotation`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const error = await t.throwsAsync(
    execAsync(`${CLI} validate ${temp} --contract StorageV2_Bad_NoAnnotation --reference StorageV1 --requireReference`),
  );
  const expectation: string[] = [`Stdout: ${(error as any).stdout}`, `Stderr: ${(error as any).stderr}`];
  t.snapshot(expectation.join('\n'));
});

test('validate - requireReference - has reference - safe', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Validate.sol:StorageV2_Ok_NoAnnotation`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const output = (
    await execAsync(
      `${CLI} validate ${temp} --contract StorageV2_Ok_NoAnnotation --reference StorageV1 --requireReference`,
    )
  ).stdout;
  t.snapshot(output);
});

test('validate - no upgradeable', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Storage088.sol:Storage088`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const output = (await execAsync(`${CLI} validate ${temp}`)).stdout;
  t.snapshot(output);
});

test('validate - ok', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Annotation.sol:Annotation`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const output = (await execAsync(`${CLI} validate ${temp}`)).stdout;
  t.snapshot(output);
});

test('validate - single contract - ok', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/cli/Annotation.sol:Annotation`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const output = (await execAsync(`${CLI} validate ${temp} --contract Annotation`)).stdout;
  t.snapshot(output);
});

test('validate - fully qualified version of ambiguous contract name', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/ValidationsSameNameSafe.sol:SameName`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const output = (
    await execAsync(`${CLI} validate ${temp} --contract contracts/test/ValidationsSameNameSafe.sol:SameName`)
  ).stdout;
  t.snapshot(output);
});

test('validate - references fully qualified version of ambiguous contract name', async t => {
  const temp = await getTempDir(t);
  const buildInfo = await artifacts.getBuildInfo(`contracts/test/ValidationsSameNameSafe.sol:SameName`);
  await fs.writeFile(path.join(temp, 'validate.json'), JSON.stringify(buildInfo));

  const output = (
    await execAsync(
      `${CLI} validate ${temp} --contract contracts/test/ValidationsSameNameSafe.sol:SameName --reference contracts/test/ValidationsSameNameUnsafe.sol:SameName`,
    )
  ).stdout;
  t.snapshot(output);
});
