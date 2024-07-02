import { upperFirst, camelCase } from 'lodash'

/**
 * Converts valid file names to valid javascript symbols and does best effort to make them readable. Example: ds-token.test becomes DsTokenTest
 */
export function normalizeName(rawName: string): string {
  const t1 = rawName.split(' ').join('-') // spaces to - so later we can automatically convert them
  const t2 = t1.replace(/^\d+/, '') // removes leading digits
  const result = upperFirst(camelCase(t2))

  if (result === '') {
    throw new Error(`Can't guess class name, please rename file: ${rawName}`)
  }

  return result
}
