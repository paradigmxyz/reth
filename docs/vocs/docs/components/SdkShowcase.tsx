interface SdkProject {
  name: string
  description: string
  loc: string
  githubUrl: string
  logoUrl?: string
  company: string
}

const projects: SdkProject[] = [
  {
    name: 'Base Node',
    description: "Coinbase's L2 scaling solution node implementation",
    loc: '~3K',
    githubUrl: 'https://github.com/base/node-reth',
    company: 'Coinbase'
  },
  {
    name: 'Bera Reth',
    description: "Berachain's high-performance EVM node with custom features",
    loc: '~1K',
    githubUrl: 'https://github.com/berachain/bera-reth',
    company: 'Berachain'
  },
  {
    name: 'Reth Gnosis',
    description: "Gnosis Chain's xDai-compatible execution client",
    loc: '~5K',
    githubUrl: 'https://github.com/gnosischain/reth_gnosis',
    company: 'Gnosis'
  },
  {
    name: 'Reth BSC',
    description: "BNB Smart Chain execution client implementation",
    loc: '~6K',
    githubUrl: 'https://github.com/loocapro/reth-bsc',
    company: 'Binance Smart Chain'
  }
]

export function SdkShowcase() {
  return (
    <div className="grid max-w-full grid-cols-1 gap-5 overflow-hidden xl:grid-cols-2">
      {projects.map((project) => (
        <div
          key={project.name}
          className="group relative min-w-0 overflow-hidden rounded-lg border border-black/10 bg-white/5 p-6 transition-colors hover:bg-black/5 dark:border-white/10 dark:bg-white/5 dark:hover:bg-white/10"
        >
          {/* LoC Badge */}
          <div className="absolute right-5 top-5 rounded-full bg-black/10 px-3 py-1 text-sm font-medium text-black dark:bg-white/10 dark:text-white">
            {project.loc} LoC
          </div>

          {/* Content */}
          <div className="space-y-3">
            <div className="min-w-0 pr-24">
              <h3 className="mb-1 break-words text-xl font-semibold leading-tight text-black dark:text-white">
                {project.name}
              </h3>
              <p className="break-words text-sm text-gray-600 dark:text-gray-400">
                {project.company}
              </p>
            </div>

            <p className="break-words text-sm leading-relaxed text-gray-700 dark:text-gray-300">
              {project.description}
            </p>

            {/* GitHub Link */}
            <a
              href={project.githubUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-2 whitespace-nowrap text-sm font-medium text-black transition-colors hover:text-gray-700 dark:text-white dark:hover:text-gray-300"
            >
              <svg className="w-4 h-4" fill="currentColor" viewBox="0 0 24 24">
                <path d="M12 0c-6.626 0-12 5.373-12 12 0 5.302 3.438 9.8 8.207 11.387.599.111.793-.261.793-.577v-2.234c-3.338.726-4.033-1.416-4.033-1.416-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.839 1.237 1.839 1.237 1.07 1.834 2.807 1.304 3.492.997.107-.775.418-1.305.762-1.604-2.665-.305-5.467-1.334-5.467-5.931 0-1.311.469-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.301 1.23.957-.266 1.983-.399 3.003-.404 1.02.005 2.047.138 3.006.404 2.291-1.552 3.297-1.23 3.297-1.23.653 1.653.242 2.874.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.609-2.807 5.624-5.479 5.921.43.372.823 1.102.823 2.222v3.293c0 .319.192.694.801.576 4.765-1.589 8.199-6.086 8.199-11.386 0-6.627-5.373-12-12-12z"/>
              </svg>
              View on GitHub
            </a>
          </div>
        </div>
      ))}
    </div>
  )
}
