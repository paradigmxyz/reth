# Upgrade Guide

This document describes breaking changes and how to upgrade. For a complete list of changes including minor and patch releases, please refer to the [changelog](CHANGELOG.md).

## v5

Upgraded to [`memdown@5.0.0`](https://github.com/Level/memdown/blob/v5.0.0/UPGRADING.md#v5):

> Support of keys & values other than strings and Buffers has been dropped. Internally `memdown` now stores keys & values as Buffers which solves a number of compatibility issues ([Level/memdown#186](https://github.com/Level/memdown/issues/186)). If you pass in a key or value that isn't a string or Buffer, it will be irreversibly stringified.

## v4

Upgraded to [`memdown@4.0.0`](https://github.com/Level/memdown/blob/v4.0.0/UPGRADING.md#v4) and (through `level-packager@5`) [`levelup@4`](https://github.com/Level/levelup/blob/v4.0.0/UPGRADING.md#v4) and [`encoding-down@6`](https://github.com/Level/encoding-down/blob/v6.0.0/UPGRADING.md#v6). Please follow these links for more information. A quick summary: range options (e.g. `gt`) are now serialized the same as keys, `{ gt: undefined }` is not the same as `{}`, nullish values are now rejected and streams are backed by [`readable-stream@3`](https://github.com/nodejs/readable-stream#version-3xx).

Support of IE10 and node 9 has been dropped.

## v3

Dropped support for node 4. No other breaking changes.
