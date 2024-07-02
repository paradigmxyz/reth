import test from 'ava';
import { artifacts } from 'hardhat';

import { validate } from './validate';
import { solcInputOutputDecoder } from './src-decoder';

test('namespace outside contract', async t => {
  const contract = 'contracts/test/NamespacedOutsideContract.sol:Example';

  const buildInfo = await artifacts.getBuildInfo(contract);
  if (buildInfo === undefined) {
    throw new Error(`Build info not found for contract ${contract}`);
  }
  const solcOutput = buildInfo.output;
  const solcInput = buildInfo.input;
  const decodeSrc = solcInputOutputDecoder(solcInput, solcOutput);
  const error = t.throws(() => validate(solcOutput, decodeSrc));
  t.assert(
    error?.message.includes(
      'contracts/test/NamespacedOutsideContract.sol:7: Namespace struct MainStorage is defined outside of a contract',
    ),
    error?.message,
  );
});
