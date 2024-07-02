import { getAnnotationArgs, getDocumentation, hasAnnotationTag } from '../../utils/annotations';
import { inferInitializable, inferUUPS } from '../../validate/query';
import { ValidateCommandError } from './error';
import { findContract } from './find-contract';
import { SourceContract } from './validations';

interface AnnotationAssessment {
  upgradeable: boolean;
  referenceName?: string;
}

export interface UpgradeabilityAssessment {
  upgradeable: boolean;
  referenceContract?: SourceContract;
  uups?: boolean;
}

export function getUpgradeabilityAssessment(
  contract: SourceContract,
  allContracts: SourceContract[],
  overrideReferenceContract?: SourceContract,
): UpgradeabilityAssessment {
  const fullContractName = contract.fullyQualifiedName;
  const contractValidation = contract.validationData[fullContractName];

  const isUUPS = inferUUPS(contract.validationData, fullContractName);

  const annotationAssessment = getAnnotationAssessment(contract);

  let referenceContract = overrideReferenceContract;
  if (referenceContract === undefined && annotationAssessment.referenceName !== undefined) {
    referenceContract = findContract(annotationAssessment.referenceName, contract, allContracts);
  }

  let isReferenceUUPS = false;
  if (referenceContract !== undefined) {
    isReferenceUUPS = inferUUPS(referenceContract.validationData, referenceContract.fullyQualifiedName);
  }

  return {
    upgradeable:
      referenceContract !== undefined ||
      annotationAssessment.upgradeable ||
      inferInitializable(contractValidation) ||
      isUUPS,
    referenceContract: referenceContract,
    uups: isReferenceUUPS || isUUPS,
  };
}

function getAnnotationAssessment(contract: SourceContract): AnnotationAssessment {
  const node = contract.node;

  if ('documentation' in node) {
    const doc = getDocumentation(node);

    const tag = 'oz-upgrades';
    const hasUpgradeAnnotation = hasAnnotationTag(doc, tag);
    if (hasUpgradeAnnotation) {
      getAndValidateAnnotationArgs(doc, tag, contract, 0);
    }

    const upgradesFrom = getUpgradesFrom(doc, contract);
    if (upgradesFrom !== undefined) {
      return {
        upgradeable: true,
        referenceName: upgradesFrom,
      };
    } else {
      return {
        upgradeable: hasUpgradeAnnotation,
      };
    }
  } else {
    return {
      upgradeable: false,
    };
  }
}

function getAndValidateAnnotationArgs(doc: string, tag: string, contract: SourceContract, expectedLength: number) {
  const annotationArgs = getAnnotationArgs(doc, tag, undefined);
  if (annotationArgs.length !== expectedLength) {
    throw new ValidateCommandError(
      `Invalid number of arguments for @custom:${tag} annotation in contract ${contract.fullyQualifiedName}.`,
      () => `Found ${annotationArgs.length}, expected ${expectedLength}.`,
    );
  }
  return annotationArgs;
}

function getUpgradesFrom(doc: string, contract: SourceContract): string | undefined {
  const tag = 'oz-upgrades-from';
  if (hasAnnotationTag(doc, tag)) {
    const annotationArgs = getAndValidateAnnotationArgs(doc, tag, contract, 1);
    return annotationArgs[0];
  } else {
    return undefined;
  }
}
