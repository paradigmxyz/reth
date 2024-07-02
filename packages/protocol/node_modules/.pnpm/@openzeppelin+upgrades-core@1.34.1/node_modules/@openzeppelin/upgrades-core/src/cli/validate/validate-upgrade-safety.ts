import { ValidationOptions, withValidationDefaults } from '../..';

import { getBuildInfoFiles } from './build-info-file';
import { getContractReports } from './contract-report';
import { findContract } from './find-contract';
import { ProjectReport, getProjectReport } from './project-report';
import { SourceContract, validateBuildInfoContracts } from './validations';

/**
 * Validation options for upgrade safety checks.
 */
export type ValidateUpgradeSafetyOptions = Omit<ValidationOptions, 'kind'> & {
  requireReference?: boolean;
};

export type SpecifiedContracts = {
  contract: SourceContract;
  reference?: SourceContract;
};

/**
 * Validates the upgrade safety of all contracts in the build info dir's build info files.
 * Only contracts that are detected as upgradeable will be validated.
 *
 * @param buildInfoDir Absolute path of build info directory, or undefined to use the default Hardhat or Foundry build-info dir.
 * @param contract The name or fully qualified name of the contract to validate. If not specified, all upgradeable contracts in the build info directory will be validated.
 * @param reference The name or fully qualified name of the reference contract to use for storage layout comparisons. Can only be used along with `contract`. If not specified, uses the `@custom:oz-upgrades-from` annotation in the contract that is being validated.
 * @param opts Validation options, or undefined to use the default validation options.
 * @returns The project report.
 */
export async function validateUpgradeSafety(
  buildInfoDir?: string,
  contract?: string,
  reference?: string,
  opts: ValidateUpgradeSafetyOptions = {},
): Promise<ProjectReport> {
  const allOpts = withCliDefaults(opts);

  const buildInfoFiles = await getBuildInfoFiles(buildInfoDir);
  const sourceContracts = validateBuildInfoContracts(buildInfoFiles);

  const specifiedContracts = findSpecifiedContracts(sourceContracts, allOpts, contract, reference);

  const contractReports = getContractReports(sourceContracts, allOpts, specifiedContracts);
  return getProjectReport(contractReports, specifiedContracts !== undefined);
}

export function findSpecifiedContracts(
  sourceContracts: SourceContract[],
  opts: Required<ValidateUpgradeSafetyOptions>,
  contractName?: string,
  referenceName?: string,
): SpecifiedContracts | undefined {
  if (contractName !== undefined) {
    return {
      contract: findContract(contractName, undefined, sourceContracts),
      reference: referenceName !== undefined ? findContract(referenceName, undefined, sourceContracts) : undefined,
    };
  } else if (referenceName !== undefined) {
    throw new Error(`The reference option can only be specified when the contract option is also specified.`);
  } else if (opts.requireReference) {
    throw new Error(`The requireReference option can only be specified when the contract option is also specified.`);
  } else {
    return undefined;
  }
}

export function withCliDefaults(opts: ValidateUpgradeSafetyOptions): Required<ValidateUpgradeSafetyOptions> {
  if (opts.requireReference && opts.unsafeSkipStorageCheck) {
    throw new Error(`The requireReference and unsafeSkipStorageCheck options cannot be used at the same time.`);
  }
  return {
    ...withValidationDefaults(opts),
    requireReference: opts.requireReference ?? false,
  };
}
