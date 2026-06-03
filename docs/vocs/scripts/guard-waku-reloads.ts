#!/usr/bin/env bun
import { readdir, readFile, writeFile } from 'fs/promises';
import { join } from 'path';

const DIST_ROOTS = ['docs/dist/public/assets', 'docs/dist/assets'];
const reloadCall = 'window.location.reload()';
const guard = `globalThis.__rethReloadOnce??=()=>{try{let e="reth:reload:"+location.pathname+":"+Array.from(document.querySelectorAll("script[src],link[rel=modulepreload][href]")).map(e=>e.src||e.href).join("|");if(sessionStorage.getItem(e))return;sessionStorage.setItem(e,"1")}catch{}location.reload()};`;

async function findJsFiles(dir: string, files: string[] = []): Promise<string[]> {
  let entries;
  try {
    entries = await readdir(dir, { withFileTypes: true });
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return files;
    throw error;
  }

  for (const entry of entries) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      await findJsFiles(path, files);
    } else if (entry.name.endsWith('.js')) {
      files.push(path);
    }
  }
  return files;
}

async function guardWakuReloads() {
  const files = (await Promise.all(DIST_ROOTS.map(dir => findJsFiles(dir)))).flat();
  let patched = 0;
  let alreadyGuarded = 0;

  for (const file of files) {
    const content = await readFile(file, 'utf8');
    if (content.includes('__rethReloadOnce') && !content.includes(reloadCall)) {
      alreadyGuarded++;
      continue;
    }
    if (!content.includes(reloadCall)) continue;

    const updated = `${guard}\n${content.split(reloadCall).join('window.__rethReloadOnce()')}`;
    await writeFile(file, updated);
    patched++;
    console.log(`Guarded Waku reloads in ${file}`);
  }

  if (alreadyGuarded > 0) {
    console.log(`Waku reloads already guarded in ${alreadyGuarded} generated asset(s).`);
  }

  if (patched === 0) {
    if (alreadyGuarded === 0) {
      console.error('No generated Waku reload calls found to guard.');
      process.exit(1);
    }
    return;
  }
}

guardWakuReloads().catch(error => {
  console.error('Failed to guard Waku reloads:', error);
  process.exit(1);
});
