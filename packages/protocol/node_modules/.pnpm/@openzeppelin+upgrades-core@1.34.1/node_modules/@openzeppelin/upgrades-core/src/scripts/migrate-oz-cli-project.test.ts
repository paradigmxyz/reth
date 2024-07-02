import os from 'os';
import test from 'ava';
import path from 'path';
import { rimraf } from 'rimraf';
import { promises as fs } from 'fs';
import { compare as compareVersions } from 'compare-versions';
import {
  migrateManifestsData,
  NetworkFileData,
  ProjectFileData,
  MigrationOutput,
  migrateLegacyProject,
} from './migrate-oz-cli-project';

const BASE_PATH = 'src/scripts';
const OPENZEPPELIN_FOLDER = '.openzeppelin';
const PROJECT_FILE = path.join(OPENZEPPELIN_FOLDER, 'project.json');
const EXPORT_FILE = 'openzeppelin-cli-export.json';
const NETWORK_FILE = path.join(OPENZEPPELIN_FOLDER, 'rinkeby.json');

let migratableData: NetworkFileData;
let migrationOutput: MigrationOutput;
let projectData: ProjectFileData;

test.before(async () => {
  migratableData = JSON.parse(await fs.readFile(path.join(BASE_PATH, 'migratable-manifest.test.json'), 'utf8'));
  projectData = JSON.parse(await fs.readFile(path.join(BASE_PATH, 'project-file.test.json'), 'utf8'));
  migrationOutput = migrateManifestsData({ rinkeby: migratableData });

  process.chdir(await fs.mkdtemp(path.join(os.tmpdir(), 'migration-test-')));
  await fs.mkdir(OPENZEPPELIN_FOLDER);
  await fs.writeFile(PROJECT_FILE, JSON.stringify(projectData, null, 2));
  await fs.writeFile(NETWORK_FILE, JSON.stringify(migratableData, null, 2));
  await migrateLegacyProject();
});

test.after(async () => {
  await rimraf(process.cwd());
});

test('transforms manifest data', async t => {
  t.snapshot(migrationOutput.newManifestsData);
});

test('produces network export data', async t => {
  t.snapshot(migrationOutput.networksExportData);
});

test('deletes project file', async t => {
  const error = await t.throwsAsync(() => fs.access(PROJECT_FILE));
  t.is(error?.message, `ENOENT: no such file or directory, access '${PROJECT_FILE}'`);
});

test('export file contains networks and compiler data', async t => {
  const actual = JSON.parse(await fs.readFile(EXPORT_FILE, 'utf8'));
  const expected = {
    networks: migrationOutput.networksExportData,
    compiler: projectData.compiler,
  };
  t.deepEqual(actual, expected);
});

test('zosversion', t => {
  const migratableDataZosVersion = { zosversion: '2.2', ...migratableData };
  delete migratableDataZosVersion.manifestVersion;
  const output = migrateManifestsData({ rinkeby: migratableDataZosVersion });
  t.true(compareVersions(output.newManifestsData.rinkeby.manifestVersion, '3.0', '>='));
});
