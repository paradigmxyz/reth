#!/usr/bin/env bun
import { readdir, copyFile, readFile, writeFile } from 'fs/promises';
import { join } from 'path';

async function fixSearchIndex() {
  const distDir = 'docs/dist';
  const vocsDir = join(distDir, '.vocs');
  
  try {
    // 1. Find the search index file
    const files = await readdir(vocsDir);
    const searchIndexFile = files.find(f => f.startsWith('search-index-') && f.endsWith('.json'));
    
    if (!searchIndexFile) {
      console.error('❌ No search index file found in .vocs directory');
      process.exit(1);
      return;
    }
    
    console.log(`📁 Found search index: ${searchIndexFile}`);
    
    // 2. Copy search index to root of dist
    const sourcePath = join(vocsDir, searchIndexFile);
    const destPath = join(distDir, searchIndexFile);
    await copyFile(sourcePath, destPath);
    console.log(`✅ Copied search index to root: ${destPath}`);
    
    // 3. Find and update all HTML and JS files that reference the search index
    const htmlFiles = await findFiles(distDir, '.html');
    const jsFiles = await findFiles(distDir, '.js');
    console.log(`📝 Found ${htmlFiles.length} HTML files and ${jsFiles.length} JS files to update`);
    
    // 4. Replace references in all files
    const allFiles = [...htmlFiles, ...jsFiles];
    for (const file of allFiles) {
      const content = await readFile(file, 'utf-8');
      
      // Replace /.vocs/search-index-*.json with /search-index-*.json
      const updatedContent = content.replace(
        /\/.vocs\/search-index-[a-f0-9]+\.json/g,
        `/${searchIndexFile}`
      );
      
      if (content !== updatedContent) {
        await writeFile(file, updatedContent);
        console.log(`  ✓ Updated ${file}`);
      }
    }
    
    console.log('✨ Search index fix complete!');
    
  } catch (error) {
    console.error('❌ Error fixing search index:', error);
    process.exit(1);
  }
}

async function findFiles(dir: string, extension: string, files: string[] = []): Promise<string[]> {
  const { readdir } = await import('fs/promises');
  const entries = await readdir(dir, { withFileTypes: true });
  
  for (const entry of entries) {
    const fullPath = join(dir, entry.name);
    
    // Skip .vocs, docs, and _site directories
    if (entry.name === '.vocs' || entry.name === 'docs' || entry.name === '_site') continue;
    
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
