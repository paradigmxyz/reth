import { promises as fs } from 'fs';
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
    
    // Extract the current crate name and module path from the file path
    // Remove the base path to get the relative path within the docs
    const relativePath = file.startsWith('./') ? file.slice(2) : file;
    const docsRelativePath = relativePath.replace(/^docs\/dist\/docs\//, '');
    const pathParts = docsRelativePath.split('/');
    const fileName = pathParts[pathParts.length - 1];
    
    // Determine if this is the root index
    const isRootIndex = pathParts.length === 1 && fileName === 'index.html';
    
    // Extract crate name - it's the first directory in the docs-relative path
    const crateName = isRootIndex ? null : pathParts[0];
    
    // Build the current module path (everything between crate and filename)
    const modulePath = pathParts.slice(1, -1).join('/');
    
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
      // Fix module links within the same crate (relative paths without ./ or ../)
      // These need to include the current crate name in the path
      .replace(/href="([^/:"\.](?:[^/:"]*)?)\/index\.html"/g, (match, moduleName) => {
        // Skip if it's already an absolute path or contains a protocol
        if (moduleName.startsWith('/') || moduleName.includes('://')) {
          return match;
        }
        // For the root index page, these are crate links, not module links
        if (isRootIndex) {
          return `href="${BASE_PATH}/${moduleName}/index.html"`;
        }
        // For module links within a crate, we need to build the full path
        // If we're in a nested module, we need to go up to the crate root then down to the target
        const fullPath = modulePath ? `${crateName}/${modulePath}/${moduleName}` : `${crateName}/${moduleName}`;
        return `href="${BASE_PATH}/${fullPath}/index.html"`;
      })
      
      // Also fix other relative links (structs, enums, traits) that don't have index.html
      .replace(/href="([^/:"\.#][^/:"#]*\.html)"/g, (match, pageName) => {
        // Skip if it's already an absolute path or contains a protocol
        if (pageName.startsWith('/') || pageName.includes('://')) {
          return match;
        }
        // Skip for root index page as it shouldn't have such links
        if (isRootIndex) {
          return match;
        }
        // For other doc pages in nested modules, build the full path
        const fullPath = modulePath ? `${crateName}/${modulePath}/${pageName}` : `${crateName}/${pageName}`;
        return `href="${BASE_PATH}/${fullPath}"`;
      })
      
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

  // Find the actual search JS filename from the HTML files
  let actualSearchJsFile = '';
  for (const htmlFile of htmlFiles) {
    const htmlContent = await fs.readFile(htmlFile, 'utf-8');
    const searchMatch = htmlContent.match(/data-search-js="[^"]*\/([^"]+)"/);
    if (searchMatch && searchMatch[1]) {
      actualSearchJsFile = searchMatch[1];
      console.log(`Found search JS file: ${actualSearchJsFile} in ${htmlFile}`);
      break;
    }
  }
  
  if (!actualSearchJsFile) {
    console.error('Could not detect search JS filename from HTML files');
    process.exit(1);
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
      // Use the detected search filename
      content = content.replace(
        /getVar\("search-js"\)/g,
        `"${actualSearchJsFile}"`
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