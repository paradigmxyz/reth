import { defineConfig } from 'vocs'
import { sidebar } from './sidebar'

export default defineConfig({
  title: 'Reth',
  sidebar,
  vite: {
    // https://vite.dev/guide/static-deploy.html#github-pages
    // Deploying to owner.github.io/reth - base must be set to '/reth
    base: '/reth',
  }
})
