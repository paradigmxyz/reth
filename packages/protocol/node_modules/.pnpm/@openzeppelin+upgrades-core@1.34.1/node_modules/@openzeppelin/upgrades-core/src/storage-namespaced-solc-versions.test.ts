import _test, { TestFn } from 'ava';
import { artifacts } from 'hardhat';

import { validate } from './validate';
import { solcInputOutputDecoder } from './src-decoder';

interface Context {
  validate: (solcVersion?: string) => ReturnType<typeof validate>;
}

const test = _test as TestFn<Context>;

test.before(async t => {
  const contract = 'contracts/test/Namespaced.sol:Example';

  const buildInfo = await artifacts.getBuildInfo(contract);
  if (buildInfo === undefined) {
    throw new Error(`Build info not found for contract ${contract}`);
  }
  const solcOutput = buildInfo.output;
  const solcInput = buildInfo.input;
  const decodeSrc = solcInputOutputDecoder(solcInput, solcOutput);

  t.context.validate = solcVersion => validate(solcOutput, decodeSrc, solcVersion, solcInput);
});

test('namespace with older solc version', async t => {
  const { validate } = t.context;
  const error = t.throws(() => validate('0.8.19'));
  t.assert(
    error?.message.includes(
      `contracts/test/Namespaced.sol: Namespace annotations require Solidity version >= 0.8.20, but 0.8.19 was used`,
    ),
    error?.message,
  );
});

test('namespace with correct solc version', async t => {
  const { validate } = t.context;
  validate('0.8.20');
  t.pass();
});

test('namespace with newer solc version', async t => {
  const { validate } = t.context;
  validate('0.8.21');
  t.pass();
});

test('namespace with no solc version', async t => {
  const { validate } = t.context;
  const error = t.throws(() => validate(undefined));
  t.assert(
    error?.message.includes(
      `contracts/test/Namespaced.sol: Namespace annotations require Solidity version >= 0.8.20, but no solcVersion parameter was provided`,
    ),
    error?.message,
  );
});
