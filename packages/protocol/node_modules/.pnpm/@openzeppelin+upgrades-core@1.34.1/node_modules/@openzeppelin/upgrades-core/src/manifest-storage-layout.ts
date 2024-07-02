import { Manifest } from './manifest';
import { isCurrentLayoutVersion } from './storage/extract';
import { StorageLayout, isStructMembers } from './storage/layout';
import { ValidationData } from './validate/data';
import { findVersionWithoutMetadataMatches, unfoldStorageLayout } from './validate/query';
import { stabilizeTypeIdentifier } from './utils/type-id';
import { DeepArray, deepEqual } from './utils/deep-array';
import { UpgradesError } from './error';

export async function getStorageLayoutForAddress(
  manifest: Manifest,
  validations: ValidationData,
  implAddress: string,
): Promise<StorageLayout> {
  const data = await manifest.read();
  const versionWithoutMetadata = Object.keys(data.impls).find(
    v => data.impls[v]?.address === implAddress || data.impls[v]?.allAddresses?.includes(implAddress),
  );
  if (versionWithoutMetadata === undefined) {
    throw new UpgradesError(
      `Deployment at address ${implAddress} is not registered`,
      () => 'To register a previously deployed proxy for upgrading, use the forceImport function.',
    );
  }
  // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
  const { layout } = data.impls[versionWithoutMetadata]!;
  if (isCurrentLayoutVersion(layout)) {
    return layout;
  } else {
    const updatedLayout = getUpdatedStorageLayout(validations, versionWithoutMetadata, layout);

    if (updatedLayout === undefined) {
      return layout;
    } else {
      await manifest.lockedRun(async () => {
        const data = await manifest.read();
        // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
        data.impls[versionWithoutMetadata]!.layout = updatedLayout;
        await manifest.write(data);
      });
      return updatedLayout;
    }
  }
}

// This function is used to retrieve the updated storage layout for an impl
// contract from the Manifest file, for which we only have the version hash
// without metadata and a storage layout. The storage layout is updated in the
// sense that it is updated to include newer information that hadn't been
// extracted when the impl contract was deployed, such as struct members.
export function getUpdatedStorageLayout(
  data: ValidationData,
  versionWithoutMetadata: string,
  layout: StorageLayout,
): StorageLayout | undefined {
  // In this map we store the candidate storage layouts, based on having the
  // same versionWithoutMetadata and a sufficiently similar layout. The keys of
  // the map are the versionWithMetadata of each contract, to avoid storing the
  // same contract multiple times.
  const sameLayout = new Map<string, StorageLayout>();

  outer: for (const [contractName, validationRun] of findVersionWithoutMetadataMatches(data, versionWithoutMetadata)) {
    const updatedLayout = unfoldStorageLayout(validationRun, contractName);

    // Check that the layout roughly matches the one we already have.
    if (updatedLayout.storage.length !== layout.storage.length) {
      continue;
    }
    if (updatedLayout.solcVersion !== layout.solcVersion) {
      continue;
    }
    for (const [i, item] of layout.storage.entries()) {
      const updatedItem = updatedLayout.storage[i];
      if (item.label !== updatedItem.label) {
        continue outer;
      }
      if (stabilizeTypeIdentifier(item.type) !== stabilizeTypeIdentifier(updatedItem.type)) {
        continue outer;
      }
    }

    const { version } = validationRun[contractName];
    if (version && !sameLayout.has(version.withMetadata)) {
      sameLayout.set(version.withMetadata, updatedLayout);
    }
  }

  if (sameLayout.size > 1) {
    // Reject multiple matches unless they all have exactly the same layout.
    const typeMembers = new Map<string, DeepArray<string>>();
    for (const { types } of sameLayout.values()) {
      for (const [typeId, item] of Object.entries(types)) {
        if (item.members === undefined) {
          continue;
        }

        const members: DeepArray<string> = isStructMembers(item.members)
          ? item.members.map(m => [stabilizeTypeIdentifier(m.type), m.label])
          : item.members;
        const stableId = stabilizeTypeIdentifier(typeId);

        if (!typeMembers.has(stableId)) {
          typeMembers.set(stableId, members);
        } else if (!deepEqual(members, typeMembers.get(stableId))) {
          return undefined;
        }
      }
    }
  }

  if (sameLayout.size > 0) {
    const [updatedLayout] = sameLayout.values();
    return updatedLayout;
  }
}
