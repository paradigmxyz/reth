import _chalk from 'chalk';
import debug from '../../utils/debug';
import {
  getContractVersion,
  getStorageLayout,
  ValidationOptions,
  withValidationDefaults,
  Version,
  ValidationData,
  ValidateUpgradeSafetyOptions,
  getErrors,
  UpgradeableContractErrorReport,
  getStorageUpgradeReport,
} from '../..';
import { Report } from '../../standalone';
import { getUpgradeabilityAssessment } from './upgradeability-assessment';
import { SourceContract } from './validations';
import { LayoutCompatibilityReport } from '../../storage/report';
import { indent } from '../../utils/indent';
import { SpecifiedContracts } from './validate-upgrade-safety';

/**
 * Report for an upgradeable contract.
 * Contains the standalone report, and if there is a reference contract, the reference contract name and storage layout report.
 */
export class UpgradeableContractReport implements Report {
  constructor(
    readonly contract: string,
    readonly reference: string | undefined,
    readonly standaloneReport: UpgradeableContractErrorReport,
    readonly storageLayoutReport: LayoutCompatibilityReport | undefined,
  ) {}

  get ok(): boolean {
    return this.standaloneReport.ok && (this.storageLayoutReport === undefined || this.storageLayoutReport.ok);
  }

  /**
   * Explain any errors in the report.
   */
  explain(color = true): string {
    const result: string[] = [];
    const chalk = new _chalk.Instance({ level: color && _chalk.supportsColor ? _chalk.supportsColor.level : 0 });
    const icon = this.ok ? chalk.green('✔') : chalk.red('✘');
    if (this.reference === undefined) {
      result.push(` ${icon}  ${this.contract}`);
    } else {
      result.push(` ${icon}  ${this.contract} (upgrades from ${this.reference})`);
    }
    if (!this.standaloneReport.ok) {
      result.push(indent(this.standaloneReport.explain(color), 6));
    }
    if (this.storageLayoutReport !== undefined && !this.storageLayoutReport.ok) {
      result.push(indent(this.storageLayoutReport.explain(color), 6));
    }
    return result.join('\n\n');
  }
}

/**
 * Gets upgradeble contract reports for the upgradeable contracts in the given set of source contracts.
 * Only contracts that are detected as upgradeable will be included in the reports.
 * Reports include upgradeable contracts regardless of whether they pass or fail upgrade safety checks.
 *
 * @param sourceContracts The source contracts to check, which must include all contracts that are referenced by the given contracts. Can also include non-upgradeable contracts, which will be ignored.
 * @param opts The validation options.
 * @param specifiedContracts If provided, only the specified contract (upgrading from its reference contract) will be reported.
 * @returns The upgradeable contract reports.
 */
export function getContractReports(
  sourceContracts: SourceContract[],
  opts: Required<ValidateUpgradeSafetyOptions>,
  specifiedContracts?: SpecifiedContracts,
) {
  const upgradeableContractReports: UpgradeableContractReport[] = [];

  const contractsToReport: SourceContract[] =
    specifiedContracts !== undefined ? [specifiedContracts.contract] : sourceContracts;

  for (const sourceContract of contractsToReport) {
    const upgradeabilityAssessment = getUpgradeabilityAssessment(
      sourceContract,
      sourceContracts,
      specifiedContracts?.reference,
    );
    if (opts.requireReference && upgradeabilityAssessment.referenceContract === undefined) {
      throw new Error(
        `The contract ${sourceContract.fullyQualifiedName} does not specify what contract it upgrades from. Add the \`@custom:oz-upgrades-from <REFERENCE_CONTRACT>\` annotation to the contract, or include the reference contract name when running the validate command or function.`,
      );
    } else if (specifiedContracts !== undefined || upgradeabilityAssessment.upgradeable) {
      const reference = upgradeabilityAssessment.referenceContract;
      const kind = upgradeabilityAssessment.uups ? 'uups' : 'transparent';
      const report = getUpgradeableContractReport(sourceContract, reference, { ...opts, kind: kind });
      if (report !== undefined) {
        upgradeableContractReports.push(report);
      }
    }
  }
  return upgradeableContractReports;
}

function getUpgradeableContractReport(
  contract: SourceContract,
  referenceContract: SourceContract | undefined,
  opts: ValidationOptions,
): UpgradeableContractReport | undefined {
  let version;
  try {
    version = getContractVersion(contract.validationData, contract.fullyQualifiedName);
  } catch (e: any) {
    if (e.message.endsWith('is abstract')) {
      // Skip abstract upgradeable contracts - they will be validated as part of their caller contracts
      // for the functions that are in use.
      return undefined;
    } else {
      throw e;
    }
  }

  debug('Checking: ' + contract.fullyQualifiedName);
  const standaloneReport = getStandaloneReport(contract.validationData, version, opts);

  let reference: string | undefined;
  let storageLayoutReport: LayoutCompatibilityReport | undefined;

  if (opts.unsafeSkipStorageCheck !== true && referenceContract !== undefined) {
    const layout = getStorageLayout(contract.validationData, version);

    const referenceVersion = getContractVersion(referenceContract.validationData, referenceContract.fullyQualifiedName);
    const referenceLayout = getStorageLayout(referenceContract.validationData, referenceVersion);

    reference = referenceContract.fullyQualifiedName;
    storageLayoutReport = getStorageUpgradeReport(referenceLayout, layout, withValidationDefaults(opts));
  }

  return new UpgradeableContractReport(contract.fullyQualifiedName, reference, standaloneReport, storageLayoutReport);
}

function getStandaloneReport(
  data: ValidationData,
  version: Version,
  opts: ValidationOptions,
): UpgradeableContractErrorReport {
  const errors = getErrors(data, version, withValidationDefaults(opts));
  return new UpgradeableContractErrorReport(errors);
}
