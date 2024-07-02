import { ValidateCommandError } from './error';
import { SourceContract } from './validations';

export class ReferenceContractNotFound extends Error {
  /**
   * The contract reference that could not be found.
   */
  readonly reference: string;

  /**
   * The fully qualified name of the contract that referenced the missing contract.
   */
  readonly origin?: string;

  constructor(reference: string, origin?: string) {
    const msg =
      origin !== undefined
        ? `Could not find contract ${reference} referenced in ${origin}.`
        : `Could not find contract ${reference}.`;
    super(msg);
    this.reference = reference;
    this.origin = origin;
  }
}

export function findContract(contractName: string, origin: SourceContract | undefined, allContracts: SourceContract[]) {
  const foundContracts = allContracts.filter(c => c.fullyQualifiedName === contractName || c.name === contractName);

  if (foundContracts.length > 1) {
    const msg =
      origin !== undefined
        ? `Found multiple contracts with name ${contractName} referenced in ${origin.fullyQualifiedName}.`
        : `Found multiple contracts with name ${contractName}.`;
    throw new ValidateCommandError(
      msg,
      () =>
        `This may be caused by old copies of build info files. Clean and recompile your project, then run the command again with the updated files.`,
    );
  } else if (foundContracts.length === 1) {
    return foundContracts[0];
  } else {
    throw new ReferenceContractNotFound(contractName, origin?.fullyQualifiedName);
  }
}
