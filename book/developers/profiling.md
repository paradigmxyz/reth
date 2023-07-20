# Profiling reth

#### Table of Contents  
 - [Latency profiling (WIP)](#latency-profiling)
 - [Memory profiling](#memory-profiling)
   - [Jemalloc](#jemalloc)
   - [Monitoring memory usage](#monitoring-memory-usage)
   - [Limiting process memory](#limiting-process-memory)
   - [Understanding allocation with jeprof](#understanding-allocation-with-jeprof)


[TODO: intro that lays out the below prior knowledge requirements]()

Audience:
 * Assuming knowledge of:
   * Linux command line maturity
   * How to compile reth from source
   * How to set up grafana + reth metrics
 * Requires:
   * Linux

## Memory profiling

<!-- TODO: i feel like this might be jumping into things too quickly - right now the document feels like it just "starts" and starts talking about memory, would like to make a more gradual
introduction so the reader has all the required context when they get to this paragraph -->
When a program consumes all of the system's available memory (and swap, if any), the OOM killer starts killing processes that are taking up the most memory, until the system has
memory available again. [todo: go here for more info on oomkiller?]().
Reth is in the class of rust programs that distributes to many different hardware targets, some with less memory than others. As a result, sometimes bugs can cause memory leaks or out-of-memory crashes for _some_ users, but not others.
Reth is also a complex program, with many moving pieces, and it can be difficult to know where to start when debugging an OOM or other memory leak.
Understanding how to profile memory usage is an extremely valuable skill when faced with this type of problem, and can quickly help shed light on the root cause of a memory leak.

In this tutorial, we will be reviewing:
 * How to monitor reth's memory usage,
 * How to emulate a low-memory environment to lab-reproduce OOM crashes,
 * How to enable `jemalloc` and its built-in memory profiling, and
 * How to use `jeprof` to interpret heap profiles and identify potential root causes for a memory leak.

### Jemalloc

[Jemalloc](https://jemalloc.net/) is a general-purpose allocator that is used [across the industry in production](https://engineering.fb.com/2011/01/03/core-data/scalable-memory-allocation-using-jemalloc/), well known for its performance benefits, predictability, and profiling capabilities.
We've seen significant performance benefits in reth when using jemalloc, but will be primarily focusing on its profiling capabilities.
Jemalloc also provides tools for analyzing and visualizing its the allocation profiles it generates, notably `jeprof`.


#### Enabling jemalloc in reth
Reth includes a `jemalloc` feature to explicitly use jemalloc instead of the system allocator:
```
cargo build --features jemalloc
```

While the `jemalloc` feature does enable jemalloc, reth has an additional feature, `profiling`, that must be used to enable heap profiling. This feature implicitly enables the `jemalloc`
feature as well:
```
cargo build --features jemalloc-prof
```

When performing a longer-running or performance-sensitive task with reth, such as a sync test or load benchmark, it's usually recommended to use the `maxperf` profile. However, the `maxperf`
profile does not enable debug symbols, which are required for tools like `perf` and `jemalloc` to produce results that a human can interpret. Reth includes a performance profile with debug symbols called `debug-fast`. To compile reth with debug symbols, jemalloc, profiling, and a performance profile:
```
cargo build --features jemalloc-prof --profile debug-fast

# May improve performance even more 
RUSTFLAGS="-C target-cpu=native" cargo build --features jemalloc-prof --profile debug-fast
```

### Monitoring memory usage

 * Reth's dashboard
  * Jemalloc metrics
  * Component memory metrics

### Limiting process memory

Memory leaks that cause OOMs can be difficult to trigger sometimes, and highly depend on the testing hardware. A testing machine with 128GB of RAM is not going to encounter OOMs caused by
memory spikes or leaks as often as a machine with only 8GB of RAM. Development machines are powerful for a reason, so artificially limiting memory usage is often the best way to replicate a
user's hardware. This can help developers debug issues that only occur on devices with limited hardware. `cgroups` is a tool that allows developers to limit the memory usage of a process,
making it extremely useful to developers in understanding how their application performs in low-memory environments.

 * How to use `cgroups` to limit memory usage
   * Enable cgroups on your system
     * grub var
   * Create a cgroup
   * Important - check swap!
   * Using `cgexec`

### Understanding allocation with jeprof

 * How to enable profiling with environment variables
   * `_RJEM_MALLOC_blah=blah`
 * What is produced by jemalloc
   * When are snapshots taken
   * `jeprof.*.heap`
 * How to visualize jemalloc heap profiles
   * jeprof
   * `--pdf`
   * flamegraphs

### TODO / to resolve
 * tips?
 * intro / background on memory?
 * write a brief description / introduction to the OOM acronym before using it all over the place
 * how can this document give the reader more intuition on debugging memory leaks beyond providing tutorials on tools

