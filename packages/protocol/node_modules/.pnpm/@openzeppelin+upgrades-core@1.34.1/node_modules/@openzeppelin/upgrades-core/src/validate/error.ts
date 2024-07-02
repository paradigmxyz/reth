import { ValidationError } from './run';
import { UpgradesError } from '../error';
import { UpgradeableContractErrorReport } from './report';

export class ValidationErrors extends UpgradesError {
  constructor(
    contractName: string,
    readonly errors: ValidationError[],
  ) {
    super(`Contract \`${contractName}\` is not upgrade safe`, () =>
      new UpgradeableContractErrorReport(errors).explain(),
    );
  }
}

export class ContractSourceNotFoundError extends UpgradesError {
  constructor() {
    super('The requested contract was not found. Make sure the source code is available for compilation');
  }
}
