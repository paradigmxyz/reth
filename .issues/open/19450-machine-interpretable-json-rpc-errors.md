---
title: machine-interpretable json-rpc errors
labels:
    - C-enhancement
    - M-prevent-stale
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.001591Z
info:
    author: "0x416e746f6e"
    created_at: 2025-11-02T18:55:45Z
    updated_at: 2025-11-24T10:30:40Z
---

### Describe the feature

presently, the errors returned by (op)-reth's (auth)rpc are not well machine-interpretable:

- the `error.code` field of the response is not uniquely mapped to the error condition
  - for example, same error code `-32000` shows up for errors like:
    - `"already known"`
    - `"nonce too low"`
    - `"exceeds block gas limit"`
    - `"replacement transaction underpriced"`
    - `"transaction underpriced"`
    - `"invalid chain ID"`
    - `"in-flight transaction limit reached for delegated accounts"`
    - `"intrinsic gas too low"`
  - making it rather useless for a meaningful logic that could respond to the error condition appropriately
- the `error.message` field might appear to be a better input-signal, however it's also not always best adapted for machine-interpretation (performance-wise) because some of the error messages are dynamically generated.  for example:
  - `"insufficient funds for gas * price + value: have 416323025940983384295 want 432928375997868000000"`
  - `"nonce too low: next nonce 13337, tx nonce 13321"`
  - `"tx fee (2287286506567664000000 wei) exceeds the configured cap (1000000000000000000 wei)"`
    - this one is really special.  if for the error messages above one _could try_ to do something using `starts_with` function, then with this particular one the static prefix is way too short for reliable switch case.
    - therefore, in order to reliably triage the error-class one would have to resort to regex pattern-matching or to a convoluted use of `starts_with` and `contains`.

therefore, the present feature-request asks for one of the following:

- either: use deterministic `error.code` values for each particular error class
- or: use static `error.message` strings for each particular error class
  - and move out the dynamic parameters into `error.data` field (a part of json-rpc spec)

^-- if any of the above would be implemented that would be really great.  if both, that would be just perfect.
