import { defineConfig } from 'vocs'
import { sidebar } from './sidebar'

export default defineConfig({
  title: 'Reth',
  logoUrl: '/reth/logo.png',
  iconUrl: '/reth/logo.png',
  ogImageUrl: '/reth/reth-prod.png',
  sidebar,
  basePath: '/reth',
  topNav: [
    { text: 'Run', link: '/run/ethereum' },
    { text: 'SDK', link: '/sdk/overview' },
    { text: 'GitHub', link: 'https://github.com/paradigmxyz/reth' },
    {
      text: 'v1.4.8',
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
  vite: {
    // https://vite.dev/guide/static-deploy.html#github-pages
    // Deploying to owner.github.io/reth - base must be set to '/reth
    base: '/reth',
  },
  theme: {
    accentColor: {
      light: '#1f1f1f',
      dark: '#ffffff',
    }
  }
})
