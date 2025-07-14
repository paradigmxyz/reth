import { promises as fs } from 'fs';
import { join, relative } from 'path';
import { glob } from 'glob';

const CARGO_DOCS_PATH = '../../target/doc';
const VOCS_DIST_PATH = './docs/dist/docs';
const BASE_PATH = '/docs';

async function injectCargoDocs() {
  console.log('Injecting cargo docs into Vocs dist...');

  // Check if cargo docs exist
  try {
    await fs.access(CARGO_DOCS_PATH);
  } catch {
    console.error(`Error: Cargo docs not found at ${CARGO_DOCS_PATH}`);
    console.error("Please run: cargo doc --no-deps --workspace --exclude 'example-*'");
    process.exit(1);
  }

  // Check if Vocs dist exists
  try {
    await fs.access('./docs/dist');
  } catch {
    console.error('Error: Vocs dist not found. Please run: bun run build');
    process.exit(1);
  }

  // Create docs directory in dist if it doesn't exist
  await fs.mkdir(VOCS_DIST_PATH, { recursive: true });

  // Copy all cargo docs to the dist/docs folder
  console.log(`Copying cargo docs to ${VOCS_DIST_PATH}...`);
  await fs.cp(CARGO_DOCS_PATH, VOCS_DIST_PATH, { recursive: true });

  // Fix relative paths in HTML files to work from /reth/docs
  console.log('Fixing relative paths in HTML files...');
  
  const htmlFiles = await glob(`${VOCS_DIST_PATH}/**/*.html`);
  
  for (const file of htmlFiles) {
    let content = await fs.readFile(file, 'utf-8');
    
    // Fix static file references
    content = content
      // CSS and JS in static.files
      .replace(/href="\.\/static\.files\//g, `href="${BASE_PATH}/static.files/`)
      .replace(/src="\.\/static\.files\//g, `src="${BASE_PATH}/static.files/`)
      .replace(/href="\.\.\/static\.files\//g, `href="${BASE_PATH}/static.files/`)
      .replace(/src="\.\.\/static\.files\//g, `src="${BASE_PATH}/static.files/`)
      
      // Fix the dynamic font loading in the script tag
      .replace(/href="\$\{f\}"/g, `href="${BASE_PATH}/static.files/\${f}"`)
      .replace(/href="\.\/static\.files\/\$\{f\}"/g, `href="${BASE_PATH}/static.files/\${f}"`)
      
      // Fix crate navigation links
      .replace(/href="\.\/([^/]+)\/index\.html"/g, `href="${BASE_PATH}/$1/index.html"`)
      .replace(/href="\.\.\/([^/]+)\/index\.html"/g, `href="${BASE_PATH}/$1/index.html"`)
      // Fix simple crate links (without ./ or ../)
      .replace(/href="([^/:"]+)\/index\.html"/g, `href="${BASE_PATH}/$1/index.html"`)
      
      // Fix root index.html links
      .replace(/href="\.\/index\.html"/g, `href="${BASE_PATH}/index.html"`)
      .replace(/href="\.\.\/index\.html"/g, `href="${BASE_PATH}/index.html"`)
      
      // Fix rustdoc data attributes
      .replace(/data-root-path="\.\/"/g, `data-root-path="${BASE_PATH}/"`)
      .replace(/data-root-path="\.\.\/"/g, `data-root-path="${BASE_PATH}/"`)
      .replace(/data-static-root-path="\.\/static\.files\/"/g, `data-static-root-path="${BASE_PATH}/static.files/"`)
      .replace(/data-static-root-path="\.\.\/static\.files\/"/g, `data-static-root-path="${BASE_PATH}/static.files/"`)
      
      // Fix search index paths
      .replace(/data-search-index-js="[^"]+"/g, `data-search-index-js="${BASE_PATH}/search-index.js"`)
      .replace(/data-search-js="([^"]+)"/g, `data-search-js="${BASE_PATH}/static.files/$1"`)
      .replace(/data-settings-js="([^"]+)"/g, `data-settings-js="${BASE_PATH}/static.files/$1"`)
      
      // Fix logo paths
      .replace(/src="\.\/static\.files\/rust-logo/g, `src="${BASE_PATH}/static.files/rust-logo`)
      .replace(/src="\.\.\/static\.files\/rust-logo/g, `src="${BASE_PATH}/static.files/rust-logo`)
      
      // Fix search functionality by ensuring correct load order
      // Add the rustdoc-vars initialization before other scripts
      .replace(/<script src="([^"]*storage[^"]*\.js)"><\/script>/g, 
        `<script src="$1"></script>`);
    
    await fs.writeFile(file, content, 'utf-8');
  }

  // Also fix paths in JavaScript files
  const jsFiles = await glob(`${VOCS_DIST_PATH}/**/*.js`);
  
  for (const file of jsFiles) {
    let content = await fs.readFile(file, 'utf-8');
    
    // Fix any hardcoded paths in JS files
    content = content
      .replace(/"\.\/static\.files\//g, `"${BASE_PATH}/static.files/`)
      .replace(/"\.\.\/static\.files\//g, `"${BASE_PATH}/static.files/`)
      .replace(/"\.\/([^/]+)\/index\.html"/g, `"${BASE_PATH}/$1/index.html"`)
      .replace(/"\.\.\/([^/]+)\/index\.html"/g, `"${BASE_PATH}/$1/index.html"`);
    
    // Fix the search form submission issue that causes page reload
    // Instead of submitting a form, just ensure the search functionality is loaded
    if (file.includes('main-') && file.endsWith('.js')) {
      content = content.replace(
        /function sendSearchForm\(\)\{document\.getElementsByClassName\("search-form"\)\[0\]\.submit\(\)\}/g,
        'function sendSearchForm(){/* Fixed: No form submission needed - search loads via script */}'
      );
      
      // Also fix the root path references in the search functionality
      content = content.replace(
        /getVar\("root-path"\)/g,
        `"${BASE_PATH}/"`
      );
      
      // Fix static-root-path to avoid double paths
      content = content.replace(
        /getVar\("static-root-path"\)/g,
        `"${BASE_PATH}/static.files/"`
      );
      
      // Fix the search-js variable to return just the filename
      content = content.replace(
        /getVar\("search-js"\)/g,
        `"search-f7877310.js"`
      );
      
      // Fix the search index loading path
      content = content.replace(
        /resourcePath\("search-index",".js"\)/g,
        `"${BASE_PATH}/search-index.js"`
      );
    }
    
    // Fix paths in storage.js which contains the web components
    if (file.includes('storage-') && file.endsWith('.js')) {
      content = content.replace(
        /getVar\("root-path"\)/g,
        `"${BASE_PATH}/"`
      );
    }
    
    await fs.writeFile(file, content, 'utf-8');
  }

  console.log('Cargo docs successfully injected!');
  console.log(`The crate documentation will be available at ${BASE_PATH}`);
}

// Run the script
injectCargoDocs().catch(console.error);