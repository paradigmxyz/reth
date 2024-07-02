import * as versions from 'compare-versions';

import { ValidationRunData } from './run';

type ValidationDataV1 = ValidationRunData;

type ValidationDataV2 = ValidationRunData[];

const currentMajor = '3';
const currentVersion = '3.4';

interface ValidationDataV3 {
  version: '3' | '3.1' | '3.2' | '3.3' | '3.4';
  log: ValidationRunData[];
}

export type ValidationDataCurrent = ValidationDataV3;

export type ValidationData = ValidationDataV1 | ValidationDataV2 | ValidationDataV3;

export function normalizeValidationData(data: ValidationData): ValidationDataCurrent {
  if (isCurrentValidationData(data, false)) {
    return data;
  } else if (Array.isArray(data)) {
    return { version: currentVersion, log: data };
  } else {
    return { version: currentVersion, log: [data] };
  }
}

export function isCurrentValidationData(data: ValidationData, exact = true): data is ValidationDataCurrent {
  if (Array.isArray(data)) {
    return false;
  } else if (!('version' in data)) {
    return false;
  } else if (typeof data.version === 'string' && versions.validate(data.version)) {
    if (exact) {
      return data.version === currentVersion;
    } else {
      return versions.compare(data.version, `${currentMajor}.*`, '=');
    }
  } else {
    throw new Error('Unknown version or malformed validation data');
  }
}

export function concatRunData(
  newRunData: ValidationRunData,
  previousData?: ValidationDataCurrent,
): ValidationDataCurrent {
  return {
    version: currentVersion,
    log: [newRunData].concat(previousData?.log ?? []),
  };
}
