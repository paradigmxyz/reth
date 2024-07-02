import { StorageLayout } from '../storage';
import { isEnumMembers } from '../storage/layout';
import { stabilizeTypeIdentifier } from './type-id';

export function stabilizeStorageLayout(layout: StorageLayout) {
  return {
    storage: layout.storage.map(s => ({ ...s, type: stabilizeTypeIdentifier(s.type) })),
    types: Object.entries(layout.types).map(([type, item]) => {
      const members =
        item.members &&
        (isEnumMembers(item.members)
          ? item.members
          : item.members.map(m => ({ ...m, type: stabilizeTypeIdentifier(m.type) })));
      return [stabilizeTypeIdentifier(type), { ...item, members }];
    }),
    namespaces: layout.namespaces
      ? Object.entries(layout.namespaces).map(([ns, items]) => {
          return [ns, items.map(item => ({ ...item, type: stabilizeTypeIdentifier(item.type) }))];
        })
      : undefined,
  };
}
