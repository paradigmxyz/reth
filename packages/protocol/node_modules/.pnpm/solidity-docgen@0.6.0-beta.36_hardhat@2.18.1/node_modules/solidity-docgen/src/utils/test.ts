import _test, { TestFn } from 'ava';
import hre from 'hardhat';
import { BuildInfo } from 'hardhat/types';
import { promises as fs } from 'fs';

interface Context {
  build: BuildInfo[];
}

const test = _test as TestFn<Context>;

test.before('reading build info', async t => {
  t.context.build = await Promise.all(
    (await hre.artifacts.getBuildInfoPaths()).map(async p => JSON.parse(await fs.readFile(p, 'utf8')))
  );
});

export default test;
