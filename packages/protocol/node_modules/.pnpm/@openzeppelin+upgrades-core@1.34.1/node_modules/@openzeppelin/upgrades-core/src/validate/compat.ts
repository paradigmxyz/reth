// Legacy interface for backwards compatibility

import { ContractValidation, ValidationRunData } from './run';

export type RunValidation = ValidationRunData;
export type ValidationLog = RunValidation[];
export type Validation = RunValidation;

export type ValidationResult = ContractValidation;
