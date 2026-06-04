import { defineConfig } from 'vocs/config'
import { sidebar } from './sidebar'
import { basePath } from './redirects.config'

export default defineConfig({
  title: 'Reth',
  description: 'Reth is a secure, performant, and modular Ethereum execution client built in Rust.',
  accentColor: 'light-dark(#1f1f1f, #ffffff)',
  srcDir: 'docs',
  logoUrl: '/logo.png',
  iconUrl: '/logo.png',
  ogImageUrl: '/reth-prod.png',
  outDir: 'docs/dist',
  renderStrategy: 'full-static',
  sidebar,
  basePath,
  search: {
    fuzzy: true
  },
  topNav: [
    { text: 'Run', link: '/run/ethereum' },
    { text: 'SDK', link: '/sdk' },
    { text: 'Rustdocs', link: '/docs' },
    { text: 'GitHub', link: 'https://github.com/paradigmxyz/reth' },
    {
      text: 'v2.2.0',
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
  editLink: {
    link: "https://github.com/paradigmxyz/reth/edit/main/docs/vocs/docs/pages/:path",
  }
})
