#!/usr/bin/env bun
import { readdir, readFile, writeFile } from 'fs/promises'
import { join } from 'path'

const ASSETS_DIR = './docs/dist/public/assets'
// Vocs's Link component, minified: `c=<usePrefetch>(!!i);if(<isExternal>(e.to))return(0,<jsxRuntime>.jsx)`.
// Minified names change between builds, so they are captured rather than hardcoded.
const LINK_MARKER = /c=([\w$]+)\(!!i\);if\(([\w$]+)\(e\.to\)\)return\(0,([\w$]+)\.jsx\)/
const RUSTDOCS_NAVIGATION_GUARD = 'if(t==="/docs"||t?.startsWith("/docs/"))return(0,'

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

    if (!LINK_MARKER.test(code)) continue

    await writeFile(
      path,
      code.replace(
        LINK_MARKER,
        (_, prefetch, isExternal, jsx) =>
          `c=${prefetch}(!!i);${RUSTDOCS_NAVIGATION_GUARD}${jsx}.jsx)(\`a\`,{...o,href:t});if(${isExternal}(e.to))return(0,${jsx}.jsx)`,
      ),
    )
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
