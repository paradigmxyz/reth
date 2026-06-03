#!/usr/bin/env bun
import { createHash } from 'node:crypto';
import { readdir, readFile, stat } from 'node:fs/promises';
import { isAbsolute, join, relative, resolve, sep } from 'node:path';
import react from '@vitejs/plugin-react';
import * as vite from 'vite';
import { resolveConfig } from 'vocs/config';
import { vocs } from 'vocs/vite';
import { SearchDocuments } from '../node_modules/vocs/dist/internal/search.js';

const HASH_INPUTS = [
  'package.json',
  'bun.lock',
  'bunfig.toml',
  'tsconfig.json',
  'vocs.config.ts',
  'redirects.config.ts',
  'sidebar.ts',
  'sidebar-cli-reth.ts',
  'types.ts',
  'docs',
  'public',
  'scripts',
];

function isIgnored(path: string) {
  const normalized = relative(process.cwd(), path).split('\\').join('/');
  return normalized === 'docs/dist' || normalized.startsWith('docs/dist/');
}

async function findFiles(path: string, files: string[] = []): Promise<string[]> {
  if (isIgnored(path)) return files;

  let metadata;
  try {
    metadata = await stat(path);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return files;
    throw error;
  }

  if (metadata.isFile()) {
    files.push(path);
    return files;
  }

  if (!metadata.isDirectory()) return files;

  const entries = await readdir(path, { withFileTypes: true });
  for (const entry of entries.sort((a, b) => compare(a.name, b.name))) {
    await findFiles(join(path, entry.name), files);
  }

  return files;
}

async function getBuildId() {
  const files = (await Promise.all(HASH_INPUTS.map(path => findFiles(path)))).flat().sort();
  const hash = createHash('sha256');

  for (const file of files) {
    hash.update(relative(process.cwd(), file));
    hash.update('\0');
    hash.update(await readFile(file));
    hash.update('\0');
  }

  return hash.digest('base64url').slice(0, 12);
}

function compare(a: string, b: string) {
  if (a < b) return -1;
  if (a > b) return 1;
  return 0;
}

function normalizeSearchDocumentId(
  id: string,
  config: Parameters<typeof SearchDocuments.fromConfig>[0],
) {
  if (id.startsWith('nav:')) return id;

  const [filePath, anchor = ''] = id.split('#');
  const pagesDir = resolve(config.rootDir, config.srcDir, config.pagesDir);
  const relativeFilePath = relative(pagesDir, filePath);

  if (relativeFilePath.startsWith('..') || isAbsolute(relativeFilePath)) return id;

  return `${relativeFilePath.split(sep).join('/')}${anchor ? `#${anchor}` : ''}`;
}

function configureDeterministicSearchDocuments() {
  const searchDocuments = SearchDocuments as {
    fromConfig: typeof SearchDocuments.fromConfig;
  };
  const fromConfig = searchDocuments.fromConfig;

  // Vocs collects search docs concurrently; sort after collection so MiniSearch IDs stay stable.
  searchDocuments.fromConfig = async config => {
    const documents = await fromConfig(config);

    return documents
      .map(document => ({
        ...document,
        id: normalizeSearchDocumentId(document.id, config),
      }))
      .sort(
        (a, b) =>
          compare(a.id, b.id) ||
          compare(a.href, b.href) ||
          compare(a.title, b.title) ||
          compare(a.type, b.type),
      );
  };
}

async function build() {
  configureDeterministicSearchDocuments();

  const config = await resolveConfig();
  const buildId = await getBuildId();
  console.log(`Using deterministic Vocs build id ${buildId}`);

  const builder = await vite.createBuilder({
    configFile: false,
    define: {
      'import.meta.env.WAKU_BUILD_ID': JSON.stringify(buildId),
    },
    plugins: [react(), vocs()],
    build: {
      outDir: config.outDir,
    },
  });

  await builder.buildApp();
}

build().catch(error => {
  console.error(error);
  process.exit(1);
});
