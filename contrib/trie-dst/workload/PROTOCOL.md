# Whole-input fuzz protocol, version 1

This directory is a self-contained workload source: a consumer fetches its Git
URL and directory and runs `prepare.sh --workload-config FILE`. No fuzz-engine
SDK, linking step, or private workload package is needed. The existing
`bedrock.yaml`, independent executable, and legacy Bedrock loop remain usable.
This protocol does not promise mid-execution snapshot or suffix-replay parity.

## Runtime manifest

Bedrock's outer preparation knows the VM assets and exports the session (the
workload's `prepare.sh` alone only builds workload artifacts):

```sh
./scripts/dst/run.sh --workload /absolute/reth/contrib/trie-dst/workload \
  --prepare-only --executor-manifest /absolute/run/fuzz-session.json
```

The resulting `fuzz-session.json` has this shape:

```json
{
  "version": 1,
  "planner": ["/absolute/prepared/reth-trie-dst"],
  "executor": ["/absolute/path/to/backend", "serve"],
  "args": {"blocks": 64, "updates": 64}
}
```

The commands are argv arrays, not shell strings, and execute from the session
manifest's directory. The exporter emits absolute executable and artifact paths.
The actual Bedrock executor argv includes `serve --` and the prepared kernel,
initrd, compose, image, planner, controller, and run configuration flags.
`args` is the workload's normalized, opaque configuration object. Native
workloads may emit the same manifest with a different executor command.

## Planner

Run `<planner argv> <hook> -`, send one JSON request on stdin, and read one JSON
value on stdout. Diagnostics go to stderr; a nonzero exit means the hook failed.
Inputs and donors below are **strings containing JSON**, not embedded objects.
Paths are arrays of string segments, including decimal array indices and the
case-sensitive `Account`/`Storage` variant names.

| Hook | Request | Response |
| --- | --- | --- |
| `describe` | `{}` | `{"protocol":"rift-fuzz","version":1,"capabilities":["seed-inputs","mutation-operators","mutation-sites","mutate-input","validate-input","reduce-input"]}` |
| `seed-inputs` | `{"args":{...}}` | Array of eight input JSON strings |
| `mutation-operators` | `{"args":{...}}` | Array of `{"name":"...","donor":true,"selector":{...}}`; donor and selector are optional |
| `mutation-sites` | `{"args":{...},"input":"...","operator":"update.remove"}` | Array of `{"operator":"update.remove","path":["operations","0","updates","0"]}` |
| `mutate-input` | `{"args":{...},"input":"...","operator":"...","path":["..."],"seed":42,"donor":"...","donor_path":["..."]}` | One input JSON string; donor is needed only for crossover |
| `validate-input` | `{"args":{...},"input":"..."}` | `{"blocks":N,"expected_roots":["0x..."]}`; invalid input exits nonzero |
| `reduce-input` | `{"args":{...},"input":"...","limit":64}` | Array of strictly smaller, valid input JSON strings |

Selectors retain the existing `path`, `kinds`, and `exclude_last` fields.
`mutation-sites` is authoritative; a selector is only a discovery optimization.
Invalid mutations leave the input unchanged. The optional `donor_path`
selects an exact donor site; without it, the mutation seed selects a
compatible donor site deterministically. The planner owns account-creation,
nonempty-batch, block-size, and final-commit validation. `validate-input` retains
its existing response and does not wrap success in a boolean envelope.

Reduction removes committed suffixes, contiguous operation ranges, and individual
updates or prewarm hints, in that deterministic order. Every returned candidate
passes the same input validator, is unique within the response, and has fewer
UTF-8 bytes than the supplied input string. Candidates are independent, not a
sequence of cumulative edits. `limit` must be a positive integer; it defaults to
64 and is capped at 1024. At most `16 * min(limit, 1024)` candidates are examined;
request size and aggregate candidate string bytes are each bounded to 32 MiB.
An empty result means no reduction was found within these bounds, not that the
input is globally minimal. The caller must re-execute candidates and retain only
those reproducing its objective; the reducer does not claim failure preservation.

## Executor

The manifest's executor is a persistent process, booting and warming its worker
once. It reads one request per stdin line and writes one response per stdout
line. Stdout is exclusively protocol data; logs go to stderr. Requests are
sequential; ids are unsigned 64-bit integers echoed unchanged in responses.

```json
{"version":1,"id":7,"method":"execute","params":{"input":"{\"operations\":[...]}","seed":42,"out":"/absolute/artifacts/7"}}
```

`out` must be an absolute, per-execution artifact directory; relative paths are
rejected. Each request executes the whole input and fully finalizes it before
responding. Success has this shape:

```json
{"version":1,"id":7,"result":{"coverage":[[123,1],[456,9]],"coverage_id":"stable-build-and-map-fingerprint","verdict":{},"failure_signature":null,"duration_ns":123456}}
```

Coverage contains raw nonzero indexed counters: index is `u32`, count is `u8`.
Indices are stable across executions of the same build/map; `coverage_id`
identifies that build and mapping. Counts are not bucketed by the workload.
`verdict` is opaque backend JSON. `failure_signature` is a string only for a real
objective verdict, otherwise null. Infrastructure failures are protocol errors,
not objective failures:

```json
{"version":1,"id":7,"error":{"code":"execution_failed","message":"diagnostic"}}
```

`{"version":1,"id":8,"method":"describe","params":{}}` returns
`{"version":1,"id":8,"result":{"protocol":"rift-fuzz-executor","version":1}}`.
`{"version":1,"id":9,"method":"shutdown","params":{}}` returns
`{"version":1,"id":9,"result":{}}`, then exits cleanly.
