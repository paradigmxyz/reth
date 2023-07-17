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

 * What is jemalloc
 * What jemalloc provides (jeprof, jemalloc metrics)
 * Compiling with the `jemalloc` feature
 * Compiling with the `jemalloc-prof` feature
 * `debug-fast` for debug symbols

### Monitoring memory usage

 * Reth's dashboard
  * Jemalloc metrics
  * Component memory metrics

### Limiting process memory

 * Why limit memory usage
 * What is `cgroups`
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

