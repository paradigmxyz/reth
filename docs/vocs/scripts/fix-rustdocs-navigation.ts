#!/usr/bin/env bun
import { readdir, readFile, writeFile } from 'fs/promises'
import { join } from 'path'

const ASSETS_DIR = './docs/dist/public/assets'
const LINK_MARKER = 'c=Yd(!!i);if(Hd(e.to))'
const RUSTDOCS_NAVIGATION_GUARD =
  'c=Yd(!!i);if(t==="/docs"||t?.startsWith("/docs/"))return(0,v.jsx)(`a`,{...o,href:t});if(Hd(e.to))'

async function fixRustdocsNavigation() {
  const files = await readdir(ASSETS_DIR)
  let patchedFiles = 0

  for (const file of files) {
    if (!file.endsWith('.js')) continue

    const path = join(ASSETS_DIR, file)
    const code = await readFile(path, 'utf-8')

    if (code.includes(RUSTDOCS_NAVIGATION_GUARD)) {
      patchedFiles++
      continue
    }

    if (!code.includes(LINK_MARKER)) continue

    await writeFile(path, code.replace(LINK_MARKER, RUSTDOCS_NAVIGATION_GUARD))
    patchedFiles++
    console.log(`Patched Rustdocs navigation in ${path}`)
  }

  if (patchedFiles !== 1) {
    console.error(`Expected to patch one Vocs client navigation bundle, patched ${patchedFiles}`)
    process.exit(1)
  }
}

fixRustdocsNavigation().catch((error) => {
  console.error('Error fixing Rustdocs navigation:', error)
  process.exit(1)
})
