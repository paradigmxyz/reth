import test, { ExecutionContext } from 'ava';
import { artifacts, run } from 'hardhat';
import {
  TASK_COMPILE_SOLIDITY_GET_SOLC_BUILD,
  TASK_COMPILE_SOLIDITY_RUN_SOLC,
  TASK_COMPILE_SOLIDITY_RUN_SOLCJS,
} from 'hardhat/builtin-tasks/task-names';

import { makeNamespacedInput } from './make-namespaced';
import { SolcBuild } from 'hardhat/types/builtin-tasks';
import { SolcInput, SolcOutput } from '../solc-api';
import { BuildInfo } from 'hardhat/types';

test('make namespaced input', async t => {
  const origBuildInfo = await artifacts.getBuildInfo('contracts/test/NamespacedToModifyImported.sol:Example');
  await testMakeNamespaced(origBuildInfo, t, '0.8.20');
});

test('make namespaced input - solc 0.7', async t => {
  // The nameNamespacedInput function must work for different solc versions, since it is called before we check whether namespaces are used with solc >= 0.8.20
  const origBuildInfo = await artifacts.getBuildInfo('contracts/test/NamespacedToModify07.sol:HasFunction');
  await testMakeNamespaced(origBuildInfo, t, '0.7.6');
});

async function testMakeNamespaced(
  origBuildInfo: BuildInfo | undefined,
  t: ExecutionContext<unknown>,
  solcVersion: string,
) {
  if (origBuildInfo === undefined) {
    throw new Error('Build info not found');
  }

  // Inefficient, but we want to test that we don't actually modify the original input object
  const origInput = JSON.parse(JSON.stringify(origBuildInfo.input));

  const modifiedInput = makeNamespacedInput(origBuildInfo.input, origBuildInfo.output);

  // Run hardhat compile on the modified input and make sure it has no errors
  const modifiedOutput = await hardhatCompile(modifiedInput, solcVersion);
  t.is(modifiedOutput.errors, undefined);

  normalizeIdentifiers(modifiedInput);
  t.snapshot(modifiedInput);

  t.deepEqual(origBuildInfo.input, origInput);
  t.notDeepEqual(modifiedInput, origInput);
}

function normalizeIdentifiers(input: SolcInput): void {
  for (const source of Object.values(input.sources)) {
    if (source.content !== undefined) {
      source.content = source.content
        .replace(/\$MainStorage_\d{1,6}/g, '$MainStorage_random')
        .replace(/\$SecondaryStorage_\d{1,6}/g, '$SecondaryStorage_random')
        .replace(/\$astId_\d+_\d{1,6}/g, '$astId_id_random');
    }
  }
}

async function hardhatCompile(input: SolcInput, solcVersion: string): Promise<SolcOutput> {
  const solcBuild: SolcBuild = await run(TASK_COMPILE_SOLIDITY_GET_SOLC_BUILD, {
    quiet: true,
    solcVersion,
  });

  if (solcBuild.isSolcJs) {
    return await run(TASK_COMPILE_SOLIDITY_RUN_SOLCJS, {
      input,
      solcJsPath: solcBuild.compilerPath,
    });
  } else {
    return await run(TASK_COMPILE_SOLIDITY_RUN_SOLC, {
      input,
      solcPath: solcBuild.compilerPath,
    });
  }
}
