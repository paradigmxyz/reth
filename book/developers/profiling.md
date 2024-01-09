# Profiling reth

#### Table of Contents
 - [Memory profiling](#memory-profiling)
   - [Jemalloc](#jemalloc)
   - [Monitoring memory usage](#monitoring-memory-usage)
   - [Limiting process memory](#limiting-process-memory)
   - [Understanding allocation with jeprof](#understanding-allocation-with-jeprof)

## Memory profiling

When a program consumes all of the system's available memory (and swap, if any), the OOM killer starts killing processes that are taking up the most memory, until the system has
memory available again. [See kernel.org for a great (although outdated) introduction to out-of-memory management.](https://www.kernel.org/doc/gorman/html/understand/understand016.html).
Reth distributes to many different hardware targets, some with less memory than others. As a result, sometimes bugs can cause memory leaks or out-of-memory crashes for _some_ users, but not others.
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

Reth's dashboard has a few metrics that are important when monitoring memory usage. The **Jemalloc memory** graph shows reth's memory usage. The *allocated* label shows the memory used by the reth process which cannot be reclaimed unless reth frees that memory. This metric exceeding the available system memory would cause reth to be killed by the OOM killer.
<img width="749" alt="Jemalloc memory" src="https://github.com/paradigmxyz/reth/assets/6798349/2653c5a2-bd7c-46a6-a593-23809389628e">

Some of reth's internal components also have metrics for the memory usage of certain data structures, usually data structures that are likely to contain many elements or may consume a lot of memory at peak load.

**The bodies downloader buffer**:
<img width="749" alt="The bodies downloader buffer graph" src="https://github.com/paradigmxyz/reth/assets/6798349/75383724-24ae-4f4f-98a9-72d01731a5f9">

**The blockchain tree block buffer**:
<img width="749" alt="The blockchain tree block buffer graph" src="https://github.com/paradigmxyz/reth/assets/6798349/7162c6d4-ed18-48c1-a327-50a245dabc95">

**The transaction pool subpools**:
<img width="749" alt="The transaction pool subpool size graph" src="https://github.com/paradigmxyz/reth/assets/6798349/c5066fd6-7ff7-4e62-9226-89327c7a802c">

One of these metrics growing beyond, 2GB for example, is likely a bug and could lead to an OOM on a low memory machine. It isn't likely for that to happen frequently, so in the best case these metrics can be used to
rule out these components from having a leak, if an OOM is occurring.

### Limiting process memory

Memory leaks that cause OOMs can be difficult to trigger sometimes, and highly depend on the testing hardware. A testing machine with 128GB of RAM is not going to encounter OOMs caused by
memory spikes or leaks as often as a machine with only 8GB of RAM. Development machines are powerful for a reason, so artificially limiting memory usage is often the best way to replicate a
user's hardware. This can help developers debug issues that only occur on devices with limited hardware. `cgroups` is a tool that allows developers to limit the memory usage of a process,
making it extremely useful to developers in understanding how their application performs in low-memory environments.

### How to use cgroups to limit process memory

In order to use cgroups to limit process memory, sometimes it must be explicitly enabled as a kernel parameter. For example, the following line is sometimes necessary to enable cgroup memory limits on
Ubuntu machines that use GRUB:
```
GRUB_CMDLINE_LINUX_DEFAULT="cgroup_enable=memory"
```
Then, create a named cgroup:
```
sudo cgcreate -t $USER:$USER -a $USER:$USER -g memory:rethMemory
```
The memory limit for the named cgroup can be set in `sys/fs/cgroup/memory`. This for example sets an 8 gigabyte memory limit:
```
echo 8G > /sys/fs/cgroup/memory/rethMemory/memory.limit_in_bytes
```
If the intention of setting up the cgroup is to strictly limit memory and simulate OOMs, a high amount of swap may prevent those OOMs from happening.
To check swap, use `free -m`:
```
ubuntu@bench-box:~/reth$ free -m
              total        used        free      shared  buff/cache   available
Mem:         257668       10695      218760          12       28213      244761
Swap:          8191         159        8032
```
If this is a problem, it may be worth either adjusting the system swappiness or disabling swap overall.

Finally, `cgexec` can be used to run reth under the cgroup:
```
cgexec -g memory:rethMemory reth node
```

### Understanding allocation with jeprof

When reth is built with the `jemalloc-prof` feature and debug symbols, the profiling still needs to be configured and enabled at runtime. This is done with the `_RJEM_MALLOC_CONF` environment variable. Take the following
command to launch reth with jemalloc profiling enabled:
```
_RJEM_MALLOC_CONF=prof:true,lg_prof_interval:32,lg_prof_sample:19 reth node
```

If reth is not built properly, you will see this when you try to run reth:
```
~/p/reth (dan/managing-memory)> _RJEM_MALLOC_CONF=prof:true,lg_prof_interval:32,lg_prof_sample:19 reth node
<jemalloc>: Invalid conf pair: prof:true
<jemalloc>: Invalid conf pair: lg_prof_interval:32
<jemalloc>: Invalid conf pair: lg_prof_sample:19
```

If this happens, jemalloc likely needs to be rebuilt with the `jemalloc-prof` feature enabled.

If everything is working, this will output `jeprof.*.heap` files while reth is running.
[The jemalloc website](http://jemalloc.net/jemalloc.3.html#opt.abort) has a helpful overview of the options available, for example `lg_prof_interval`, `lg_prof_sample`, `prof_leak`, and `prof_final`.

Now that we have the heap snapshots, we can analyze them using `jeprof`. An example of jeprof usage and output can be seen on the jemalloc github repository: https://github.com/jemalloc/jemalloc/wiki/Use-Case:-Leak-Checking
