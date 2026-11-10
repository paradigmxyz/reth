# Blocking executor runtime

`reth-executor` supplies the blocking execution primitives used by Reth's task,
engine, trie, storage, pipeline, EVM, and network workers. The default backend uses
Rayon, OS threads, and Crossbeam channels. The `fuzz` feature selects Shuttle, a
concurrency-testing runtime with resumable logical-thread stacks, a fixed
round-robin scheduler, and a virtual clock. The exported pool, channel, thread, lock, and clock types are selected at compile time by
that feature; call sites need no runtime trait or backend type parameter.

## Entering deterministic execution

`reth_executor::Runtime` owns a simulated execution when `fuzz` is enabled.
Create its channel, thread, lock, and clock primitives and run synchronous work inside
one explicit `Runtime::run` entry point. Finish or join spawned work before that entry point
returns. Each invocation is a separate scheduler execution; its handles and primitives
cannot be reused in another invocation. The native
backend runs the closure directly, so the same caller compiles with either backend.

```rust
use reth_executor::{channel, thread, time::Duration, Runtime};

let value = Runtime::new().run(|| {
    let (send, receive) = channel::bounded(1);
    let worker = thread::spawn(move || {
        thread::sleep(Duration::from_millis(2));
        send.send(42).expect("receiver remains connected");
    });
    let value = receive.recv().expect("producer sends a value");
    worker.join().expect("producer completes");
    value
});
assert_eq!(value, 42);
```

The supported surface follows Rayon's explicit-pool API: detached jobs, `install`,
`join`, borrowed pool/thread scopes, broadcasts, and Reth's parallel-iterator
operations. Pure compute iterators execute serially with `fuzz`. Channels support
cloned senders and receivers, bounded/unbounded queues, zero-capacity rendezvous,
blocking/nonblocking operations, receive timeouts, and disconnection. Receive
selection prioritizes branches in their written order under `fuzz`;
`reth_executor::channel::select!` and `select_biased!` support one to eight receive
branches and an optional immediate default, not send branches or timed defaults.
Native `select_biased!` also prioritizes written order; native `select!` randomizes
among ready branches.

Use the selected `thread`, `sync`, and `time` modules for blocking coordination.
Sleeps and receive deadlines suspend logical tasks rather than blocking the host
thread. Virtual time advances to the earliest deadline when none of the workload's logical threads is runnable. With no runnable work or pending timer, the runtime reports a deadlock.
A standard-library/Crossbeam blocking wait inside a simulated task can block the
host instead, so it must not be used for coordination within that execution.

## Reth integration and limits

Reth's task-management crate reexports this crate as `reth_tasks::executor`. Named workers, stateful compute pools,
work/result queues, and the engine's synchronous proof/trie channels use the
selected backend. Participating crates expose a `fuzz` feature and forward it to
their executor dependencies. Cargo feature unification selects one backend across
the dependency graph.

Tokio remains native, including explicitly Tokio-backed blocking APIs. Network
input, filesystem/database internals, resource metrics, external caches, and signer recovery implemented in the external
`reth-primitives-traits` crate are outside this scheduler. This is not whole-node deterministic replay. Repeatability
requires identical workload inputs and waits on this crate's primitives inside one `Runtime::run`. Enabling `fuzz` does not create that entry point: with `fuzz`, these primitives
used from Tokio/OS workers outside `Runtime::run` panic rather than silently falling back to
native execution. Tokio blocking workers do not inherit a simulation: when entering
from Tokio, create the primitives and run the entire synchronous workload inside
one `Runtime::run` closure on that worker. Do not move simulated handles to other
Tokio workers.

## Executable checks

The `reth-tasks/rayon` feature exposes its compute-pool APIs; `fuzz` selects their
deterministic implementation. From the repository root:

```sh
cargo run -p reth-executor --example trace
cargo run -p reth-executor --features fuzz --example trace
cargo run -p reth-tasks --features rayon --example executor
cargo run -p reth-tasks --features rayon,fuzz --example executor
```

The `trace` example checks channel handshakes, backpressure, receive selection,
timeouts, and equality of recorded send/receive events across repeated simulated
runs. The `executor` example uses Reth's task APIs for named-worker reuse, multiple
producers/consumers, borrowed scopes, and virtual time. It checks that non-`Send`
worker state is created, accessed, and dropped on its original physical thread,
and that a native Tokio task still completes with either backend.
