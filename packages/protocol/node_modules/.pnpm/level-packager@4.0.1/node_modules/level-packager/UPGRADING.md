# Upgrade Guide

This document describes breaking changes and how to upgrade. For a complete list of changes including minor and patch releases, please refer to the [changelog](CHANGELOG.md).

## v4

The `test.js` file was rewritten to test the `level-packager` api and is no longer part of the api. Implementations based on `level-packager` should instead use the tests in the `abstract/` folder.

## v3

Dropped support for node 4. No other breaking changes.
