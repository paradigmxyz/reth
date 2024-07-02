#!/usr/bin/env node
import { migrateLegacyProject } from '@openzeppelin/upgrades-core';

migrateLegacyProject().catch(e => {
  console.error(e);
  process.exit(1);
});
