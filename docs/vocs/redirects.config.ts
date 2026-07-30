export const redirects: Record<string, string> = {
  '/intro': '/overview',
  // Installation redirects
  '/installation/installation': '/installation/overview',
  '/binaries': '/installation/binaries',
  '/docker': '/installation/docker',
  '/source': '/installation/source',
  // Run a node redirects
  '/run/run-a-node': '/run/overview',
  '/run/mainnet': '/run/ethereum',
  '/run/private-testnet': '/run/private-testnets',
  '/run/observability': '/run/monitoring',
  '/run/config': '/run/configuration',
  '/run/transactions': '/run/faq/transactions',
  '/run/snapshots': '/run/storage/snapshots',
  '/run/ethereum/snapshots': '/run/storage/snapshots',
  '/run/pruning': '/run/storage/pruning',
  '/run/faq/pruning': '/run/storage/pruning',
  '/run/storage-v2': '/run/storage',
  '/run/faq/storage-v2': '/run/storage',
  '/run/minimal': '/run/storage/minimal',
  '/run/ports': '/run/faq/ports',
  '/run/troubleshooting': '/run/faq/troubleshooting',
  // SDK
  '/sdk/overview': '/sdk',
  // Exex
  '/developers/exex': '/exex/overview',
  '/developers/exex/how-it-works': '/exex/how-it-works',
  '/developers/exex/hello-world': '/exex/hello-world',
  '/developers/exex/tracking-state': '/exex/tracking-state',
  '/developers/exex/remote': '/exex/remote',
  // Contributing
  '/developers/contribute': '/introduction/contributing',
}

export const basePath = '/';
