# Upgrade Guide

This document describes breaking changes and how to upgrade. For a complete list of changes including minor and patch releases, please refer to the [changelog](CHANGELOG.md).

## 1.0.0

This is the initial release of `level-transcoder`, which was forked from `level-codec`. Ultimately `level-transcoder` got a completely different API, so the two modules are not interchangeable. That said, here are the high-level differences from `level-codec` just for the record:

- Throws if an encoding is not found, rather than falling back to `'id'` encoding
- The `'binary'` encoding has been renamed to `'buffer'`, with `'binary'` as an alias
- The `'utf8'` encoding of `level-codec` did not touch Buffers. In `level-transcoder` the same encoding will call `buffer.toString('utf8')` for consistency. Consumers can use the `'buffer'` encoding to avoid this conversion.
- The `'id'` encoding (aliased as `'none'`) which wasn't supported by any active `abstract-leveldown` implementation, has been removed.
- The `'ascii'`, `'ucs2'` and `'utf16le'` encodings are not supported.
