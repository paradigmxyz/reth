# Running a native Reth node under Hermit

This experiment runs Reth's ordinary node launcher, Tokio/Rayon workers, HTTP RPC,
transaction pool, engine, and database under [Hermit](https://github.com/facebookexperimental/hermit).
Hermit controls execution at the operating-system boundary. The guest does not use
the cooperative Commonware executor from [PR #27017](https://github.com/paradigmxyz/reth/pull/27017).

The original experiment was based on [PR #27017](https://github.com/paradigmxyz/reth/pull/27017),
commit `1be64bcdaf641fdfdec8429d45f87e43b3e0dbc9`. The workload follows that branch's
use of fixed signed transactions, explicit block timestamps, and checks across the
memory-to-disk boundary.

## Running independently on main

The workload and runner also build on main commit
`2117377448154812b935d53deaf1a80783c06195` without any commits from #27017.
The Rust guest source is unchanged. No production source or dependency manifest
changes were needed. The native control produced the same chain transcript as
the original DST-based experiment.

The syscall-only campaign passed for seeds 1, 2, and 3, including Hermit's standard
verifier and independent exact-output/schedule-event repeats. All three schedules
differed, while all chain transcripts matched the native control. Evidence is in
`target/hermit-dst/main-no-pmu/` in the original checkout. The experiment worktree
is `/tmp/reth-hermit-main`, branch `alexey/hermit-main`.

When running from a checkout under `/tmp`, explicitly pass `--output-dir` pointing
to a new directory outside `/tmp`; Hermit replaces the guest's `/tmp` mount.

The PMU-enabled seed-1 campaign also passed, including Hermit's two-run standard
verifier and an independent repeat with byte-identical stdout/stderr and identical
complete schedule JSON. It recorded 63,550 events, 2,613 scheduler turns, and 62
threads. The chain transcript matched the native control. Evidence is in
`target/hermit-dst/main-pmu/`; the 900-second invocation limit accommodated the
verifier. This establishes that the Hermit demonstration does not require the
DST branch, for both scheduling profiles exercised here. Nightly formatting and
all-feature integration-test Clippy also passed.

## Observed result

The initial campaign, using `--no-pmu`, passed on September 7, 2026. All three seeds completed the real node
workload and matched the native control's chain transcript. For each seed,
Hermit's standard verifier passed and two independent runs had byte-identical
stdout/stderr and identical ordered thread/operation events.

| Scheduler seed | Native threads | Scheduler turns | Recorded events | Schedule SHA-256 prefix |
| --- | ---: | ---: | ---: | --- |
| 1 | 61 | 3,318 | 28,830 | `9443327a7a51` |
| 2 | 61 | 3,265 | 28,758 | `3e40c1efaa2c` |
| 3 | 61 | 3,257 | 28,886 | `ed0af8dcd99e` |

Each seed executes four times: two ordinary runs plus the verifier's two runs.
The 12 node executions and probes took about 44 seconds wall time. All result
JSON files had SHA-256
`1b320abc7f78e46046c19bea5b7aa0ab2b9ebe650c718f6e6b72a77d5c58276a`.
The local evidence is in `target/hermit-dst/calibrated-campaign/`, including
`results.json`, `commands.json`, and per-seed output/schedule files.

The guest was compiled with Rust 1.96.1, `CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_DEV_SPLIT_DEBUGINFO=off`, and `CARGO_INCREMENTAL=0`. Its binary hash
was `874c42a67a7fdcb744652737958119df18d4191c41f2ae420a359623ab96e366`;
the Hermit binary hash was
`f72d73bc5b251df0784a27a6f9b97fd82808d8767f345bf24ad7979008abad9f`.
Changing binaries can change the recorded schedules.

## PMU enablement follow-up

After applying the host's rr Zen workaround, the same Hermit binary passed PMU
startup validation. A CPU-only loop of 10 million iterations completed with 100
`inbound timer preemption event` log entries at a 1 ms virtual maximum timeslice;
99 completed slices had zero syscalls and zero signals. The expected sum was
`49999995000000`. Local source, executable, and diagnostic evidence are retained
in `target/hermit-dst/pmu-probe/`.

The PMU-enabled node workload then passed twice with scheduler seed 1, matching
the native chain transcript. Both runs had byte-identical stdout/stderr and
identical complete schedule JSON: 63,323 events, 675,270,496 conditional branches,
2,572 scheduler turns, and 62 threads. The first execution took 197.7 seconds.
`target/hermit-dst/pmu-campaign/pmu-repeat-verification.json` records the independent
comparison. Hermit's separate two-run verifier exceeded the old 300-second host
limit during its second execution, so the three-seed campaign remains incomplete;
its `status.json` correctly records failure. The runner now defaults to 900 seconds
per invocation to accommodate PMU overhead. This follow-up establishes exact
seed-1 repeatability, but does not claim built-in verifier success or a completed
three-seed PMU campaign.

## Workload

The ignored integration test `hermit::hermit_native_node_transfers` starts a node
and a loopback HTTP client in the same process. It submits 15 signed transfers via
`eth_sendRawTransaction`, commits three blocks through `testing_commitBlockV1`, and
checks transaction order, canonical parents, receipts, gas usage, fees, balances,
and the sender nonce. It waits for the background database worker to persist at
least two blocks, rereads the chain, and drains graceful shutdown tasks.

Each run emits a `RETH_HERMIT_RESULT` JSON transcript containing block hashes,
state roots, receipt roots, transaction hashes, and final balances. Inputs are
fixed while Hermit varies scheduling. Matching chain results across seeds is the
application invariant; repeated-run verification separately checks reproducibility.

Discovery, bootnodes, NAT, IPC, and inbound/outbound peer slots are disabled. All
mutable guest data belongs in Hermit's fresh `/tmp`. Running the client outside
Hermit, sharing a database between repetitions, or accessing a live chain would
introduce uncontrolled inputs.

## Build Hermit

Use x86-64 Linux with user, PID, and mount namespaces, parent-child ptrace, and
seccomp support. Hermit needs nightly Rust and the libunwind/LZMA development
packages. On Debian or Ubuntu:

```sh
sudo apt-get install libunwind-dev liblzma-dev
git clone https://github.com/rrnewton/hermit.git hermit
cd hermit
git checkout f6c836b18dac01a7dc632449d5069ffeb6a3efdc
cargo rustc --release -p hermit --bin hermit --no-default-features -- -C link-arg=-llzma
```

The maintained fork is recommended by the upstream README. The upstream revision
`2c609dd1d392b3d37471a367b750d702b304fe2c` failed to build because experimental
backend modules lacked corresponding Cargo dependencies. Maintained revision
`471a14f9374e7d4c05c96a6da18ff7c5e4de64b5` builds, but its Reverie notifier calls
`pidfd_open(..., PIDFD_THREAD)`, which returns `EINVAL` on the development host's
Linux 6.8 kernel even for a `/bin/true` guest. The older revision above pins
Reverie to `22791b2fe7a837da233758ddb8f546b0c8ee07aa`, which uses the compatible
`waitid` notifier. The explicit LZMA link resolves the distribution's libunwind
dependencies. These are build/version choices; the setup does not patch Hermit's
determinization implementation.

## Native control run

Build the guest outside Hermit:

```sh
CFLAGS=-DMDBX_SAFE4QEMU=1 cargo test --locked -p reth-node-ethereum --test it \
  hermit::hermit_native_node_transfers -- \
  --exact --ignored --nocapture --test-threads=1
```

The native control passed on the development host: three blocks, 15 successful
transfers, sender nonce 15, recipient balance 1,500 wei, and background persistence.
Its final block and state root were:

```text
block: 0x0be2a0d622f46555817f048ead7d21ce0669274ee07adaabd7be702e42b847e0
state: 0xef064400f6f5002e79e1e054f3290cdd55ac6eea83413c9ef2512aefed18ccc4
```

Native success alone does not establish deterministic execution. The Hermit
campaign must also complete and its verification artifacts must be checked.

`MDBX_SAFE4QEMU` is an existing libmdbx build option. It avoids priority-inheritance
mutexes and OFD file locks; Hermit's futex model does not implement
`FUTEX_UNLOCK_PI`, which glibc probes during MDBX's default mutex initialization.
The flag changes the build configuration without editing vendored libmdbx sources.
Use the same configuration for the native control and Hermit guest.

## Run a seed campaign

From the Reth checkout, point the runner at the compiled Hermit binary:

```sh
CFLAGS=-DMDBX_SAFE4QEMU=1 \
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_DEV_SPLIT_DEBUGINFO=off CARGO_INCREMENTAL=0 \
HERMIT_BIN=/absolute/path/to/hermit/target/release/hermit \
HERMIT_SRC=/absolute/path/to/hermit \
  python3 scripts/hermit-dst.py --verification=standard --no-virtualize-cpuid 1 2 3
```

The runner discovers the test executable through Cargo's JSON build output.
Set `GUEST_BIN` to reuse a previously compiled executable. Pass
`--native-output /path/to/native.stdout` to additionally compare against a saved
native control run. `--timeout` sets a host wall-clock limit for each Hermit
invocation; a timeout terminates the process group and fails the campaign.

For each seed, the runner compares two ordinary runs and performs Hermit's
two-run `--verify` check. The guest RNG seed remains zero.
`--chaos --sched-heuristic=random` varies thread scheduling. By default the runner
also enables PMU preemption with `--max-timeslice=200000000` (200 virtual
milliseconds), RCB-based time, `--clock-multiplier=1`, and
`--panic-on-rcb-overshoot`. This allows timer interrupts inside CPU-bound code.
Hermit diagnostics are retained separately from guest stderr, and detected PMU
validation failures or unavailable counters fail the campaign.

The development host is an AMD EPYC 4585PX running Linux 6.8. Before enabling PMU,
apply the official [rr Zen workaround](https://github.com/rr-debugger/rr/wiki/Zen)
on the host and confirm that `AmdSpecLockMapShouldBeDisabled` no longer appears.
CPUID faulting remains unavailable on this kernel, so the command explicitly
exposes the host's CPUID results. Reproduction is tied to the same CPU features;
this does not establish CPU-model independence.

To reproduce the initial syscall-only campaign, pass `--no-pmu`. That explicit
profile disables PMU preemption and RCB time and sets `--clock-multiplier=0.002`
to cancel this Hermit revision's implicit 500-fold clock scaling. It explores
syscall/thread scheduling points without preempting CPU-only stretches. Both
profiles retain the fixture's ordinary RPC and persistence deadlines.

Artifacts in `target/hermit-dst/campaign-*` include exact commands, exit codes,
binary/source/lockfile hashes, source snapshots, native and guest transcripts,
schedule events, Hermit's summaries, verification JSON, and guest/verifier output.
This older runtime deletes matched internal verification logs; its comparison
counts and verdict remain in the captured verifier stderr. Newer runtimes with
`--keep-logs` additionally retain both internal logs.
The compatible runtime supports standard verification, which compares guest
stdout/stderr/status and filtered, normalized deterministic logs. Independent
same-seed comparisons retain exact stdout/stderr and ordered schedule events.
The runner also requires identical application transcripts across all seeds. Multiple seeds
must produce different ordered thread/operation event sequences; changing seed
metadata alone does not count as schedule exploration.

On a compatible newer runtime, the runner's default `--verification=strict` mode
requires `verified` and `bitwise_parity` from `--verify-strict --verify-json`.
That policy includes syscall output hashes with host addresses canonicalized;
it is stronger than the legacy runtime's filtered log comparison. Neither mode
is a byte-for-byte comparison of process memory or database files. A passing finite campaign is
evidence for this workload, binary, runtime, and host configuration; it does not
establish determinism for every Reth path.
