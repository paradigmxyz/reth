import type { Kzg } from '../depInterfaces'

function kzgNotLoaded(): never {
  throw Error('kzg library not loaded')
}

// eslint-disable-next-line import/no-mutable-exports
export let kzg: Kzg = {
  freeTrustedSetup: kzgNotLoaded,
  loadTrustedSetup: kzgNotLoaded,
  blobToKzgCommitment: kzgNotLoaded,
  computeAggregateKzgProof: kzgNotLoaded,
  verifyKzgProof: kzgNotLoaded,
  verifyAggregateKzgProof: kzgNotLoaded,
}

/**
 * @param kzgLib a KZG implementation (defaults to c-kzg)
 * @param trustedSetupPath the full path (e.g. "/home/linux/devnet4.txt") to a kzg trusted setup text file
 */
export function initKZG(kzgLib: Kzg, trustedSetupPath: string) {
  kzg = kzgLib
  kzg.loadTrustedSetup(trustedSetupPath)
}
