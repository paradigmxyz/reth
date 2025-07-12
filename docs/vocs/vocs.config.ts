import React from 'react'
import { defineConfig } from 'vocs'
import { sidebar } from './sidebar'
import { basePath } from './redirects.config'

export default defineConfig({
  title: 'Reth',
  logoUrl: '/logo.png',
  iconUrl: '/logo.png',
  ogImageUrl: '/reth-prod.png',
  sidebar,
  basePath,
  topNav: [
    { text: 'Run', link: '/run/ethereum' },
    { text: 'SDK', link: '/sdk/overview' },
    { 
      element: React.createElement('a', { href: '/docs', target: '_self' }, 'Rustdocs')
    },
    { text: 'GitHub', link: 'https://github.com/paradigmxyz/reth' },
    {
      text: 'v1.5.1',
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
    pattern: "https://github.com/paradigmxyz/reth/edit/main/book/vocs/docs/pages/:path",
  }
})
