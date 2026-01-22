---
title: Improve / Extend Documentation on using reth as a library
labels:
    - A-sdk
    - C-docs
    - M-prevent-stale
assignees:
    - jenpaff
milestone: milestone-1
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 13874
synced_at: 2026-01-21T11:32:15.970979Z
info:
    author: ckoopmann
    created_at: 2023-10-04T05:58:32Z
    updated_at: 2025-10-13T17:31:40Z
---

### Describe the change

I couldn't find too much documentation using reth as a library. (most of the docs seems to understandably be focused on the node operator).

For context currently, when I want to import reth I just add the following line to my Cargo.toml:
```
reth = { git = "https://github.com/paradigmxyz/reth" }
```

However afaik this has a few drawbacks:
1. My project might break if there is a breaking change in reth. (ofc I can fix it to a given commit, but then I miss out on all bugfixes etc. Is there any way currently to do some more "advanced" config that allows me to keep up to date with minor fixes but avoid breaking changes ? )
2. Afaik cargo will download the whole crate even if I just use some libraries, which might lead to bloat.

I'd suggest adding a chapter in the [book](https://paradigmxyz.github.io/reth/intro.html) that answers the following questions:

1. How to install reth as a library
2. Example of how to install / use only selected modules / features.
3. General philosophy / aspiration for reth as a library
4. What to expect in terms of api stability / breaking changes.
5. Current state / future plans for crates.io publish (There seems to be a [published crate](https://crates.io/crates/reth) but it's only 841B in size and currently not maintained, so I'd assume it is a placeholder. In that case it'd be good to inform users of that fact so they don't waste time trying to install reth from crates.io)

Additionally you might also want to mirror some of that info on the crates.io documentation / readme, which is currently empty.

### Additional context

_No response_
