# Troubleshooting

This page tries to answer how to deal with the most popular issues.

## Database

### Slow database inserts and updates

If you're:
1. Running behind the tip
2. Have slow canonical commit time according to the `Canonical Commit Latency time` chart on [Grafana dashboard](./observability.md#prometheus--grafana) (more than 2-3 seconds)
3. Seeing warnings in your logs such as 
   ```console
   2023-11-08T15:17:24.789731Z  WARN providers::db: Transaction insertion took too long block_number=18528075 tx_num=2150227643 hash=0xb7de1d6620efbdd3aa8547c47a0ff09a7fd3e48ba3fd2c53ce94c6683ed66e7c elapsed=6.793759034s
   ```

then most likely you're experiencing issues with the [database freelist](https://github.com/paradigmxyz/reth/issues/5228).
To confirm it, check if the values on the `Freelist` chart on [Grafana dashboard](./observability.md#prometheus--grafana)
is greater than 10M.

Currently, there are two main ways to fix this issue.


#### Compact the database
It will take around 5-6 hours and require **additional** disk space located on the same or different drive
equal to the [freshly synced node](../installation/installation.md#hardware-requirements).

1. Clone Reth
   ```bash
   git clone https://github.com/paradigmxyz/reth
   cd reth
   ```
2. Build database debug tools
   ```bash
   make db-tools
   ```
3. Run compaction (this step will take 5-6 hours, depending on the I/O speed)
   ```bash
   ./db-tools/mdbx_copy -c $(reth db path) reth_compact.dat
   ```
4. Stop Reth
5. Backup original database
   ```bash
   mv $(reth db path)/mdbx.dat reth_old.dat
   ```
6. Move compacted database in place of the original database
   ```bash
   mv reth_compact.dat $(reth db path)/mdbx.dat
   ```
7. Start Reth
8. Confirm that the values on the `Freelist` chart is near zero and the values on the `Canonical Commit Latency time` chart
is less than 1 second.
9. Delete original database
   ```bash
   rm reth_old.dat
   ```

#### Re-sync from scratch
It will take the same time as initial sync.

1. Stop Reth
2. Drop the database using [`reth db drop`](../cli/reth/db/drop.md)
3. Start reth

### Database write error

If you encounter an irrecoverable database-related errors, in most of the cases it's related to the RAM/NVMe/SSD you use. For example:
```console
Error: A stage encountered an irrecoverable error.

Caused by:
   0: An internal database error occurred: Database write error code: -30796
   1: Database write error code: -30796
```

or

```console
Error: A stage encountered an irrecoverable error.

Caused by:
   0: An internal database error occurred: Database read error code: -30797
   1: Database read error code: -30797
```

1. Check your memory health: use [memtest86+](https://www.memtest.org/) or [memtester](https://linux.die.net/man/8/memtester). If your memory is faulty, it's better to resync the node on different hardware.
2. Check database integrity:
    ```bash
    git clone https://github.com/paradigmxyz/reth
    cd reth
    make db-tools
    ./db-tools/mdbx_chk $(reth db path)/mdbx.dat | tee mdbx_chk.log
    ```
    If `mdbx_chk` has detected any errors, please [open an issue](https://github.com/paradigmxyz/reth/issues) and post the output from the `mdbx_chk.log` file.

### Concurrent database access error (using containers/Docker)

If you encounter an error while accessing the database from multiple processes and you are using multiple containers or a mix of host and container(s), it is possible the error is related to `PID` namespaces. You might see one of the following error messages.

```console
mdbx:0: panic: Assertion `osal_rdt_unlock() failed: err 1' failed.
```
or

```console
pthread_mutex_lock.c:438: __pthread_mutex_lock_full: Assertion `e != ESRCH || !robust' failed
```

If you are using Docker, a possible solution is to run all database-accessing containers with `--pid=host` flag.

For more information, check out the `Containers` section in the [libmdbx README](https://github.com/erthink/libmdbx#containers).

## Hardware Performance Testing

If you're experiencing degraded performance, it may be related to hardware issues. Below are some tools and tests you can run to evaluate your hardware performance.

If your hardware performance is significantly lower than these reference numbers, it may explain degraded node performance. Consider upgrading your hardware or investigating potential issues with your current setup.

### Disk Speed Testing with [IOzone](https://linux.die.net/man/1/iozone)

1. Test disk speed:
   ```bash
   iozone -e -t1 -i0 -i2 -r1k -s1g /tmp
   ```
   Reference numbers (on Latitude c3.large.x86):

   ```console
   Children see throughput for  1 initial writers  =  907733.81 kB/sec
   Parent sees throughput for  1 initial writers   =  907239.68 kB/sec
   Children see throughput for  1 rewriters        = 1765222.62 kB/sec
   Parent sees throughput for  1 rewriters         = 1763433.35 kB/sec
   Children see throughput for 1 random readers    = 1557497.38 kB/sec
   Parent sees throughput for 1 random readers     = 1554846.58 kB/sec
   Children see throughput for 1 random writers    =  984428.69 kB/sec
   Parent sees throughput for 1 random writers     =  983476.67 kB/sec
   ```
2. Test disk speed with memory-mapped files:
   ```bash
   iozone -B -G -e -t1 -i0 -i2 -r1k -s1g /tmp
   ```
   Reference numbers (on Latitude c3.large.x86):

   ```console
   Children see throughput for  1 initial writers  =   56471.06 kB/sec
   Parent sees throughput for  1 initial writers   =   56365.14 kB/sec
   Children see throughput for  1 rewriters        =  241650.69 kB/sec
   Parent sees throughput for  1 rewriters         =  239067.96 kB/sec
   Children see throughput for 1 random readers    = 6833161.00 kB/sec
   Parent sees throughput for 1 random readers     = 5597659.65 kB/sec
   Children see throughput for 1 random writers    =  220248.53 kB/sec
   Parent sees throughput for 1 random writers     =  219112.26 kB/sec
    ```

### RAM Speed and Health Testing

1. Check RAM speed with [lshw](https://linux.die.net/man/1/lshw):
   ```bash
   sudo lshw -short -C memory
   ```
   Look for the frequency in the output. Reference output:

   ```console
   H/W path              Device          Class          Description
   ================================================================
   /0/24/0                               memory         64GiB DIMM DDR4 Synchronous Registered (Buffered) 3200 MHz (0.3 ns)
   /0/24/1                               memory         64GiB DIMM DDR4 Synchronous Registered (Buffered) 3200 MHz (0.3 ns)
   ...
   ```

2. Test RAM health with [memtester](https://linux.die.net/man/8/memtester):
   ```bash
   sudo memtester 10G
   ```
   This will take a while. You can test with a smaller amount first:

   ```bash
   sudo memtester 1G 1
   ```
   All checks should report "ok".
