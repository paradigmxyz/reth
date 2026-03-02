import React from 'react'
import { defineConfig } from 'vocs'
import { sidebar } from './sidebar'
import { basePath } from './redirects.config'

export default defineConfig({
  title: 'Reth',
  description: 'Reth is a secure, performant, and modular Ethereum execution client built in Rust.',
  logoUrl: '/logo.png',
  iconUrl: '/logo.png',
  ogImageUrl: '/reth-prod.png',
  sidebar,
  basePath,
  search: {
    fuzzy: true
  },
  topNav: [
    { text: 'Run', link: '/run/ethereum' },
    { text: 'SDK', link: '/sdk' },
    {
      element: React.createElement('a', { href: '/docs', target: '_self' }, 'Rustdocs')
    },
    { text: 'GitHub', link: 'https://github.com/paradigmxyz/reth' },
    {
      text: 'v1.11.1',
      items: [
        {
          text: 'Releases',
          link: 'https://github.com/paradigmxyz/reth/releases'
        },
        {
          text: 'Contributing',
          link: 'https://github.com/paradigmxyz/reth/blob/main/CONTRIBUTING.md'
        }
      ]
    }
  ],
  socials: [
    {
      icon: 'github',
      link: 'https://github.com/paradigmxyz/reth',
    },
    {
      icon: 'telegram',
      link: 'https://t.me/paradigm_reth',
    },
  ],
  sponsors: [
    {
      name: 'Collaborators',
      height: 120,
      items: [
        [
          {
            name: 'Paradigm',
            link: 'https://paradigm.xyz',
            image: 'https://raw.githubusercontent.com/wevm/.github/main/content/sponsors/paradigm-light.svg',
          },
          {
            name: 'Ithaca',
            link: 'https://ithaca.xyz',
            image: 'https://raw.githubusercontent.com/wevm/.github/main/content/sponsors/ithaca-light.svg',
          }
        ]
      ]
    }
  ],
  theme: {
    accentColor: {
      light: '#1f1f1f',
      dark: '#ffffff',
    }
  },
  editLink: {
    pattern: "https://github.com/paradigmxyz/reth/edit/main/docs/vocs/docs/pages/:path",
  },
  vite: {
    plugins: [
      {
        name: 'transform-summary-links',
        apply: 'serve', // only during dev for faster feedback
        enforce: 'pre',
        async load(id) {
          if (id.endsWith('pages/cli/SUMMARY.mdx') || id.endsWith('pages/cli/summary.mdx')) {
            const { readFileSync } = await import('node:fs')
            let code = readFileSync(id, 'utf-8')
            code = code.replace(/\]\(\.\/([^)]+)\.mdx\)/g, '](/cli/\$1)')
            return code
          }
        }
      },
      {
        name: 'transform-summary-links-build',
        apply: 'build', // only apply during build
        enforce: 'pre',
        async load(id) {
          if (id.endsWith('pages/cli/SUMMARY.mdx') || id.endsWith('pages/cli/summary.mdx')) {
            const { readFileSync } = await import('node:fs')
            let code = readFileSync(id, 'utf-8')
            code = code.replace(/\]\(\.\/([^)]+)\.mdx\)/g, '](/cli/\$1)')
            return code
          }
        }
      }
    ]
  }
})
