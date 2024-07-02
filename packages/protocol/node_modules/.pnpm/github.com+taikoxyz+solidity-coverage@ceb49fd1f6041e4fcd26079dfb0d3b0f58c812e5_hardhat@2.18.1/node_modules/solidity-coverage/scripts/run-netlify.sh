#!/usr/bin/env bash
#
# E2E: runs sc-forks/hegic/contracts-v1 coverage and publishes html report in netlify
#      for CI deployment preview. This is the netlify "build" step.
#

set -o errexit

# Clone target
rm -rf contracts-v1
git clone https://github.com/sc-forks/contracts-v1.git
cd contracts-v1

# Install solidity-coverage @ current commit
yarn
yarn add --dev https://github.com/sc-forks/solidity-coverage.git#$COMMIT_REF

cat package.json

npm run coverage
