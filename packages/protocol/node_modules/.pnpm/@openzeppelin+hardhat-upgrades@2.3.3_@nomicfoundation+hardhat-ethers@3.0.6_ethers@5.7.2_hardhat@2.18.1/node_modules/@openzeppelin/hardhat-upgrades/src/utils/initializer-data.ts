import { UpgradesError } from '@openzeppelin/upgrades-core';
import { Interface } from 'ethers';

export function getInitializerData(
  contractInterface: Interface,
  args: unknown[],
  initializer?: string | false,
): string {
  if (initializer === false) {
    return '0x';
  }

  const allowNoInitialization = initializer === undefined && args.length === 0;
  initializer = initializer ?? 'initialize';

  const fragment = contractInterface.getFunction(initializer);
  if (fragment === null) {
    if (allowNoInitialization) {
      return '0x';
    } else {
      throw new UpgradesError(
        `The contract has no initializer function matching the name or signature: ${initializer}`,
        () =>
          `Ensure that the initializer function exists, specify an existing function with the 'initializer' option, or set the 'initializer' option to false to omit the initializer call.`,
      );
    }
  } else {
    return contractInterface.encodeFunctionData(fragment, args);
  }
}
