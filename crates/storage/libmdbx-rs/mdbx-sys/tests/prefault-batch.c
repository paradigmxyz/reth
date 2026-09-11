/* SPDX-License-Identifier: Apache-2.0
 * From mdbx-sys: cc -std=c11 -O1 -g -pthread tests/prefault-batch.c -o
 * /tmp/mdbx-prefault-batch
 */
#ifndef _GNU_SOURCE
#define _GNU_SOURCE 1
#endif
#include <stddef.h>
#include <sys/mman.h>
#include <sys/uio.h>
#include <unistd.h>
static int test_mincore(void *address, size_t length, void *vector);
static ssize_t test_pwrite(int fd, const void *data, size_t bytes,
                           off_t offset);
static ssize_t test_pwritev(int fd, const struct iovec *iov, int count,
                            off_t offset);
#define mincore test_mincore
#define pwrite test_pwrite
#define pwritev test_pwritev
#include "../libmdbx/mdbx.c"
#undef mincore
#undef pwrite
#undef pwritev

static char *mapping;
static size_t writes;
static unsigned fail_write;
static unsigned resident_pgno;
static size_t db_pagesize;

static int test_mincore(void *address, size_t length, void *vector) {
  const size_t offset = (char *)address - mapping;
  for (size_t i = 0; i < length / globals.sys_pagesize; ++i)
    ((unsigned char *)vector)[i] =
        (offset + i * globals.sys_pagesize) / db_pagesize == resident_pgno;
  return 0;
}

static ssize_t test_pwrite(int fd, const void *data, size_t bytes,
                           off_t offset) {
  ++writes;
  if (fail_write == 1) {
    errno = EIO;
    return -1;
  }
  return pwrite(fd, data, fail_write == 2 ? bytes / 2 : bytes, offset);
}

static ssize_t test_pwritev(int fd, const struct iovec *iov, int count,
                            off_t offset) {
  ++writes;
  if (fail_write == 1) {
    errno = EIO;
    return -1;
  }
  return pwritev(fd, iov, fail_write == 2 ? 1 : count, offset);
}

/* expected is the number of contiguous pages that may be written. */
static void check(size_t len, unsigned gap, unsigned resident, bool gc,
                  bool loose, unsigned db_log, unsigned os_log, unsigned end,
                  unsigned fail, size_t expected) {
  MDBX_env env = {0};
  MDBX_txn txn = {0};
  MDBX_cursor mc = {0};
  tree_t dbs[CORE_DBS] = {0};
  lck_t lck = {0};
  FILE *file = tmpfile();
  assert(file);
  env.ps = db_pagesize = (size_t)1 << db_log;
  env.ps2ln = db_log;
  globals.sys_pagesize = (size_t)1 << os_log;
  globals.sys_pagesize_ln2 = os_log;
  env.flags = txn.flags = MDBX_WRITEMAP;
  env.lck = &lck;
  env.lazy_fd = fileno(file);
  env.dxb_mmap.base = mapping;
  env.dxb_mmap.current = (size_t)end << db_log;
  env.page_auxbuf = calloc(3, env.ps);
  assert(env.page_auxbuf);
  txn.env = &env;
  txn.dbs = dbs;
  txn.front_txnid = 1;
  txn.tw.prefault_write_activated = true;
  txn.tw.loose_count = loose;
  txn.geo.first_unallocated = 256;
  txn.tw.repnl = pnl_alloc(len ? len : 1);
  assert(txn.tw.repnl);
  for (pgno_t pgno = 9; MDBX_PNL_GETSIZE(txn.tw.repnl) < len; ++pgno)
    if (pgno != gap)
      assert(pnl_insert_span(&txn.tw.repnl, pgno, 1) == MDBX_SUCCESS);
  pgno_t saved[32];
  assert(len < ARRAY_LENGTH(saved));
  memcpy(saved, txn.tw.repnl, MDBX_PNL_SIZEOF(txn.tw.repnl));
  mc.txn = &txn;
  mc.tree = &dbs[gc ? FREE_DBI : MAIN_DBI];
  memset(lck.mincore_cache.begin, -1, sizeof(lck.mincore_cache.begin));
  resident_pgno = resident;
  char *page = malloc(env.ps);
  assert(page);
  memset(page, 0x5a, env.ps);
  for (size_t i = 0; i < 32; ++i)
    assert(fwrite(page, env.ps, 1, file) == 1);
  assert(fflush(file) == 0);
  writes = 0;
  fail_write = fail;
  const pgr_t result = page_alloc_finalize(&env, &txn, &mc, 8, 1);
  assert(result.err == MDBX_SUCCESS);
  assert(writes == 1);
  assert(memcmp(saved, txn.tw.repnl, MDBX_PNL_SIZEOF(txn.tw.repnl)) == 0);
  for (size_t i = 0; i < 32; ++i) {
    assert(pread(env.lazy_fd, page, env.ps, i * env.ps) == (ssize_t)env.ps);
    for (size_t j = 0; j < env.ps; ++j)
      assert(page[j] ==
             (((!fail && i >= 8 && i < 8 + expected) || (fail == 2 && i == 8))
                  ? 0
                  : 0x5a));
  }
  if (fail) {
    for (size_t i = 0; i < ARRAY_LENGTH(lck.mincore_cache.begin); ++i)
      assert(lck.mincore_cache.begin[i] == P_INVALID);
    fail_write = false;
    assert(!mincore_probe(&env, 8));
  } else {
    /* Later allocations consume their original entries without another write.
     */
    for (size_t i = 1; i < expected; ++i) {
      assert(repnl_get_single(&txn) == 8 + i);
      assert(page_alloc_finalize(&env, &txn, &mc, 8 + (pgno_t)i, 1).err ==
             MDBX_SUCCESS);
      assert(writes == 1);
    }
  }
  free(page);
  free(env.page_auxbuf);
  pnl_free(txn.tw.repnl);
  fclose(file);
}

int main(void) {
  mapping = calloc(256, 65536);
  assert(mapping);
  check(10, 0, 0, false, false, 12, 12, 256, false, 8);
  check(10, 11, 0, false, false, 12, 12, 256, false, 3);
  check(10, 0, 11, false, false, 12, 12, 256, false, 3);
  check(0, 0, 0, false, false, 12, 12, 256, false, 1);
  check(10, 0, 0, true, false, 12, 12, 256, false, 1);
  check(10, 0, 0, false, true, 12, 12, 256, false, 1);
  check(10, 0, 0, false, false, 12, 14, 256, false, 1);
  check(10, 0, 0, false, false, 14, 12, 256, false, 1);
  check(10, 0, 0, false, false, 14, 14, 256, false, 2);
  check(10, 0, 0, false, false, 16, 16, 256, false, 1);
  check(10, 0, 0, false, false, 12, 12, 11, false, 3);
  check(10, 0, 0, false, false, 12, 12, 256, true, 8);
  check(10, 0, 0, false, false, 12, 12, 256, 2, 8);
  free(mapping);
  puts("bounded reclaimed-page prefault, guards, cache reuse, and write "
       "failure tests passed");
  return 0;
}
