---
title: Add a better CLI doc description of `--engine.memory-block-buffer-target`
labels:
    - A-cli
    - C-docs
    - D-good-first-issue
assignees:
    - 0xSooki
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.008937Z
info:
    author: Rjected
    created_at: 2025-11-24T19:22:58Z
    updated_at: 2025-11-26T17:34:43Z
---

### Describe the feature

The comment for this flag:
https://github.com/paradigmxyz/reth/blob/d55d641225798442f774249158d43a796e7e87da/crates/node/core/src/args/engine.rs#L24

does not completely explain what this flag actually does internally, we should document this, specifically how it relates to persistence and `--engine.persistence-threshold`

### Additional context

_No response_
