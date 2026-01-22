## Summary

Removes the `AccountsTrieChangeSets` and `StoragesTrieChangeSets` database tables along with the `MerkleChangeSets` stage and pruner segment. These tables were no longer being populated and are not needed.

## Changes

### Removed
- **Stage**: `MerkleChangeSets` stage
- **Pruner segment**: `MerkleChangeSets` pruner
- **Database tables**: `AccountsTrieChangeSets` and `StoragesTrieChangeSets` table definitions
- **Types**: `TrieChangeSetsEntry`, `BlockNumberHashedAddress`, and related trie changeset types
- **Trait methods**: `write_trie_changesets`, `clear_trie_changesets` from `TrieWriter`/`StorageTrieWriter` traits
- **Documentation**: References to the removed tables and stage

### Added
- **`DatabaseEnv::drop_orphan_table(name)`**: New method to drop tables by name without requiring compile-time type definitions. Returns `Ok(true)` if table existed and was dropped, `Ok(false)` if not found.
- **Automatic orphan table cleanup**: During `init_db`, orphaned tables are automatically dropped with an info log message.

### Kept (for compatibility)
- **`StageId::MerkleChangeSets`**: Marked as deprecated for checkpoint compatibility
- **`PruneSegment::MerkleChangeSets`**: Marked as deprecated for checkpoint compatibility

## Database Migration

Existing databases with the orphaned tables will have them automatically dropped on first startup after this update. Users will see log messages like:

```
INFO reth::db: Dropped orphaned database table table="AccountsTrieChangeSets"
INFO reth::db: Dropped orphaned database table table="StoragesTrieChangeSets"
```

## Testing

- Added `db_drop_orphan_table` test to verify the new orphan table deletion mechanism
- Verified compilation across dependent crates
