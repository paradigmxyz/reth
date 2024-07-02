import test from 'ava';
import { artifacts } from 'hardhat';
import { FunctionDefinition } from 'solidity-ast';
import { findAll, astDereferencer } from 'solidity-ast/utils';

import { getFunctionSignature } from './function';
import { SolcOutput } from '../solc-api';

testContract('ContractFunctionSignatures');
testContract('LibraryFunctionSignatures');

function testContract(contractName: string) {
  test(contractName, async t => {
    const fileName = 'contracts/test/FunctionSignatures.sol';
    const buildInfo = await artifacts.getBuildInfo(`${fileName}:${contractName}`);
    if (buildInfo === undefined) {
      throw new Error('Build info not found');
    }

    const solcOutput: SolcOutput = buildInfo.output;

    // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
    const signatures = Object.keys(solcOutput.contracts[fileName][contractName].evm.methodIdentifiers!);

    const functions: Record<string, FunctionDefinition> = {};
    for (const def of findAll('FunctionDefinition', solcOutput.sources[fileName].ast)) {
      functions[def.name] = def;
    }
    const deref = astDereferencer(solcOutput);

    for (const signature of signatures) {
      const name = signature.replace(/\(.*/, '');
      t.is(getFunctionSignature(functions[name], deref), signature);
    }
  });
}
