import React from 'react'

interface TrustedCompany {
  name: string
  logoUrl: string
}

const companies: TrustedCompany[] = [
  {
    name: 'Flashbots',
    logoUrl: '/flashbots.png'
  },
  {
    name: 'Coinbase',
    logoUrl: '/coinbase.png'
  },
  {
    name: 'Alchemy',
    logoUrl: '/alchemy.png'
  },
  {
    name: 'Succinct Labs',
    logoUrl: '/succinct.png'
  }
]

export function TrustedBy() {
  return (
    <div className="grid grid-cols-2 md:grid-cols-4 gap-6">
      {companies.map((company, index) => (
        <div
          key={index}
          className="relative bg-white/5 dark:bg-white/5 border border-black/10 dark:border-white/10 rounded-xl p-8 hover:bg-black/5 dark:hover:bg-white/10 transition-colors flex flex-col items-center justify-center h-40 group"
        >
          {/* Company Logo */}
          <div className={`flex items-center justify-center ${
            company.name === 'Coinbase' || company.name === 'Alchemy' ? 'w-32 h-32' : 'w-24 h-24'
          }`}>
            <img
              src={company.logoUrl}
              alt={`${company.name} logo`}
              className="max-w-full max-h-full object-contain"
            />
          </div>
        </div>
      ))}
    </div>
  )
}