#!/usr/bin/env bun
import { writeFileSync, mkdirSync } from 'fs'
import { join, dirname } from 'path'
import { redirects, basePath } from '../redirects.config'
// Base path for the site

function generateRedirectHtml(targetPath: string): string {
  return `<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <title>Redirecting...</title>
    <meta http-equiv="refresh" content="0; URL=${targetPath}">
    <link rel="canonical" href="${targetPath}">
</head>
<body>
    <script>
        window.location.href = "${targetPath}";
    </script>
    <p>Reth mdbook has been migrated to new docs. If you are not redirected please <a href="${targetPath}">click here</a>.</p>
</body>
</html>`
}

// Generate redirect files
Object.entries(redirects).forEach(([from, to]) => {
  // Add base path to target if it doesn't already have it
  const finalTarget = to.startsWith(basePath) ? to : `${basePath}${to}`
  
  // Remove base path if present in from path
  const fromPath = from.replace(/^\/reth\//, '')
  
  // Generate both with and without .html
  const paths = [fromPath]
  if (!fromPath.endsWith('.html')) {
    paths.push(`${fromPath}.html`)
  }
  
  paths.forEach(path => {
    const filePath = join('./docs/dist', path)
    if (!path.includes('.')) {
      // It's a directory path, create index.html
      const indexPath = join('./docs/dist', path, 'index.html')
      mkdirSync(dirname(indexPath), { recursive: true })
      writeFileSync(indexPath, generateRedirectHtml(finalTarget))
    } else {
      // It's a file path
      mkdirSync(dirname(filePath), { recursive: true })
      writeFileSync(filePath, generateRedirectHtml(finalTarget))
    }
  })
})

console.log('Redirects generated successfully!')