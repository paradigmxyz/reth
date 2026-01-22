---
title: 'improv: it is possible to optimise disk access patterns during pipeline sync?'
labels:
    - C-enhancement
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.018791Z
info:
    author: "0x416e746f6e"
    created_at: 2026-01-13T12:47:42Z
    updated_at: 2026-01-19T22:00:40Z
---

### Describe the feature

**TL;DR:**

is it possible to implement some optimisations (like read-ahead-and-in-parallel) to the staged sync pipeline of `reth`?

for example, in the `execution` stage can the next block be read in parallel with processing of the current one?

similarly, in the `merkle` stage, can the inputs for the next chunk range be read in parallel with the processing of the current chunk range?

**CONTEXT**

current implementation of pipeline sync is highly linear and sequential.

most of the stages (except `headers` and `bodies` that are write-only) behave like follows:

- iteratively read data from disk on a single thread
- process it (on the same thread)
- at interevals write back the updates to the disk (mostly the same thread, with a bit maybe on couple of others)

some of the stages behave more like pumps:

- read/process/write simultaneously, but still on a the same thread

this makes reth's sync-up process to be highly dependent on 2 factors:

- random disk read latency (on large chains in-memory buffers from the OS become rather useless)
- single CPU core performance


### Additional context

here's what typical utilisation looks like on a quite powerful cloud VM with the best-grade persistent disk attached to it (0.5ms disk read latency).

**CPU**

```
top - 12:11:16 up 4 days, 16:49,  6 users,  load average: 1.05, 1.26, 1.21
Threads: 903 total,   1 running, 902 sleeping,   0 stopped,   0 zombie
%Cpu(s):  0.5 us,  0.1 sy,  0.0 ni, 97.7 id,  1.8 wa,  0.0 hi,  0.0 si,  0.0 st
MiB Mem : 354685.4 total,   5042.4 free,  14841.1 used, 334801.8 buff/cache
MiB Swap:      0.0 total,      0.0 free,      0.0 used. 337455.0 avail Mem


   PID USER      PR  NI    VIRT    RES    SHR S  %CPU  %MEM     TIME+ COMMAND
179091 op-reth   20   0   10.4t 299.7g 296.7g D  21.5  86.5 365:09.41 tokio-runtime-w
174089 op-reth   20   0   10.4t 299.7g 296.7g S   0.7  86.5  11:47.38 tokio-runtime-w
174118 op-reth   20   0   10.4t 299.7g 296.7g S   0.3  86.5  61:42.36 reth-rayon-11
174059 op-reth   20   0   10.4t 299.7g 296.7g S   0.0  86.5   0:00.09 op-reth
```

**IO**

```
Total DISK READ:         6.51 M/s | Total DISK WRITE:         0.00 B/s
Current DISK READ:       6.51 M/s | Current DISK WRITE:     285.21 K/s
    TID  PRIO  USER     DISK READ  DISK WRITE  SWAPIN     IO>    COMMAND
 179091 be/4 op-reth     6.51 M/s    0.00 B/s  ?unavailable?  op-reth node --authrpc.addr 0.0.0.0 --authrpc~-ws.origins * --ws.port 8646 [tokio-runtime-w]
```
