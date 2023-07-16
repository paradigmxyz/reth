# Profiling reth

#### Table of Contents  
 - [Latency profiling (WIP)](#latency-profiling)
 - [Memory profiling](#memory-profiling)
  - [Jemalloc](#jemalloc)
  - [Monitoring memory usage](#monitoring-memory-usage)
  - [Limiting process memory](#limiting-process-memory)
  - [Understanding allocation with jeprof](#understanding-allocation-with-jeprof)

Audience:
* Assuming knowledge of:
 * Linux command line maturity
 * How to compile reth from source
 * How to set up grafana + reth metrics
* Requires:
 * Linux

## Memory profiling

* Why memory profile in general
* How does something OOM
* How can memory profiling

### Jemalloc

* What is jemalloc
* What jemalloc provides (jeprof, jemalloc metrics)
* Compiling with the `jemalloc` feature
* Compiling with the `jemalloc-prof` feature

### Monitoring memory usage

* Reth's dashboard
 * Jemalloc metrics
 * Component memory metrics

### Limiting process memory

* Why limit memory usage
* What is `cgroups`
* How to use `cgroups` to limit memory usage
* How to make sure the cgroup limits memory usage

### Understanding allocation with jeprof

* How to enable profiling with environment variables
* What is produced by jemalloc
* How to visualize jemalloc heap profiles

### TODO / to resolve
* tips?
* how can this document give the reader more intuition on debugging memory leaks beyond providing tutorials on tools

