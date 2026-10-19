#!/usr/bin/env bun
import { createHash } from 'crypto';
import { mkdir, readdir, readFile, unlink, writeFile } from 'fs/promises';
import { join } from 'path';

type SearchIndex = {
  documentIds?: Record<string, string>;
  storedFields?: Record<string, { href?: unknown }>;
  [key: string]: unknown;
};

async function fixSearchIndex() {
  const distDir = 'docs/dist';
  const publicDir = join(distDir, 'public');
  const publicAssetsDir = join(publicDir, 'assets');
  
  try {
    const source = await findSearchIndex(distDir, publicAssetsDir);
    if (!source) {
      console.error('No Vocs search index file found');
      process.exit(1);
      return;
    }

    console.log(`Found search index: ${join(source.dir, source.file)}`);

    await mkdir(publicAssetsDir, { recursive: true });
    const sourcePath = join(source.dir, source.file);
    let jsonText = await readFile(sourcePath, 'utf-8');
    const json = JSON.parse(jsonText) as SearchIndex;
    const normalizedCount = normalizeDocumentIds(json);
    jsonText = JSON.stringify(json);

    const hash = createHash('md5').update(jsonText).digest('hex').slice(0, 12);
    const searchIndexFile = `search-index-${hash}.json`;
    const destPath = join(publicAssetsDir, searchIndexFile);
    await writeFile(destPath, jsonText);
    await removeStaleSearchIndexes(publicAssetsDir, searchIndexFile);

    console.log(`Wrote public search index: ${destPath}`);
    if (normalizedCount > 0) {
      console.log(`Normalized ${normalizedCount} search result ids`);
    }

    const htmlFiles = await findFiles(publicDir, '.html');
    const jsFiles = await findFiles(publicDir, '.js');
    console.log(`Found ${htmlFiles.length} HTML files and ${jsFiles.length} JS files to update`);

    const allFiles = [...htmlFiles, ...jsFiles];
    for (const file of allFiles) {
      const content = await readFile(file, 'utf-8');
      const updatedContent = content.replace(
        /\/(?:\.vocs|assets)\/search-index-[A-Za-z0-9_-]+\.json/g,
        `/assets/${searchIndexFile}`
      );
      
      if (content !== updatedContent) {
        await writeFile(file, updatedContent);
        console.log(`  Updated ${file}`);
      }
    }
    
    console.log('Search index fix complete!');
    
  } catch (error) {
    console.error('Error fixing search index:', error);
    process.exit(1);
  }
}

async function removeStaleSearchIndexes(publicAssetsDir: string, keepFile: string): Promise<void> {
  const files = await readdir(publicAssetsDir);
  await Promise.all(
    files
      .filter((file) => file.startsWith('search-index-') && file.endsWith('.json') && file !== keepFile)
      .map((file) => unlink(join(publicAssetsDir, file))),
  );
}

async function findSearchIndex(
  distDir: string,
  publicAssetsDir: string,
): Promise<{ dir: string; file: string } | undefined> {
  const candidateDirs = [
    publicAssetsDir,
    join(distDir, 'server', 'assets'),
    join(distDir, '.vocs'),
    join(distDir, 'assets'),
  ];

  for (const dir of candidateDirs) {
    let files: string[];
    try {
      files = await readdir(dir);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') continue;
      throw error;
    }

    const file = files.find((entry) => entry.startsWith('search-index-') && entry.endsWith('.json'));
    if (file) return { dir, file };
  }
}

function normalizeDocumentIds(searchIndex: SearchIndex): number {
  const { documentIds, storedFields } = searchIndex;
  if (!documentIds || !storedFields) return 0;

  let normalizedCount = 0;
  for (const [id, currentId] of Object.entries(documentIds)) {
    const href = storedFields[id]?.href;
    if (typeof href !== 'string' || href.length === 0 || currentId === href) continue;

    documentIds[id] = href;
    normalizedCount++;
  }

  return normalizedCount;
}

async function findFiles(dir: string, extension: string, files: string[] = []): Promise<string[]> {
  const { readdir } = await import('fs/promises');
  const entries = await readdir(dir, { withFileTypes: true });
  
  for (const entry of entries) {
    const fullPath = join(dir, entry.name);
    
    // Skip injected rustdocs and internal build directories
    if (entry.name === 'docs' || entry.name === '_site') continue;
    
    if (entry.isDirectory()) {
      files = await findFiles(fullPath, extension, files);
    } else if (entry.name.endsWith(extension)) {
      files.push(fullPath);
    }
  }
  
  return files;
}

// Run the fix
fixSearchIndex().catch(console.error);
