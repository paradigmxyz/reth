/* SPDX-License-Identifier: Apache-2.0
 * From mdbx-sys: cc -std=c11 -O1 -g -pthread tests/prefault-runs.c -o
 * /tmp/mdbx-prefault-runs
 */
#ifndef _GNU_SOURCE
#define _GNU_SOURCE 1
#endif
#include <stddef.h>
#include <sys/mman.h>
static int test_mincore(void *address, size_t length, void *vector);
#define mincore test_mincore
#include "../libmdbx/mdbx.c"
#undef mincore

static char *mapping;
static bool resident[256];
static int test_mincore(void *address, size_t length, void *vector) {
  const size_t first = ((char *)address - mapping) / 4096;
  for (size_t i = 0; i < length / 4096; ++i)
    ((unsigned char *)vector)[i] = resident[first + i];
  return 0;
}

static void check(size_t count, unsigned scenario) {
  MDBX_env env = {0};
  MDBX_txn txn = {0};
  lck_t lck = {0};
  FILE *file = tmpfile();
  assert(file);
  env.ps = globals.sys_pagesize = 4096;
  env.ps2ln = globals.sys_pagesize_ln2 = 12;
  env.flags = txn.flags = MDBX_WRITEMAP;
  env.lck = &lck;
  env.lazy_fd = fileno(file);
  env.dxb_mmap.base = mapping;
  env.dxb_mmap.current = 256 * 4096;
  env.page_auxbuf = calloc(3, 4096);
  assert(env.page_auxbuf);
  txn.env = &env;
  txn.front_txnid = 1;
  txn.tw.prefault_write_activated = true;
  txn.geo.first_unallocated = 256;
  memset(lck.mincore_cache.begin, -1, sizeof(lck.mincore_cache.begin));
  memset(resident, 1, sizeof(resident));
  for (size_t i = 0; i < count; ++i)
    resident[4 + i] = scenario == 0   ? false
                      : scenario == 1 ? (i % 2 != 0)
                      : scenario == 2 ? true
                                      : (i == 0 || i + 1 == count);
  char page[4096];
  memset(page, 0x5a, sizeof(page));
  for (size_t i = 0; i < count + 8; ++i)
    assert(fwrite(page, sizeof(page), 1, file) == 1);
  assert(fflush(file) == 0);
  const pgr_t result = page_alloc_finalize(&env, &txn, nullptr, 4, count);
  assert(result.err == MDBX_SUCCESS);
  for (size_t i = 0; i < count + 8; ++i) {
    assert(pread(env.lazy_fd, page, sizeof(page), i * 4096) == sizeof(page));
    for (size_t j = 0; j < sizeof(page); ++j)
      assert(page[j] == (resident[i] ? 0x5a : 0));
  }
  free(env.page_auxbuf);
  fclose(file);
}

int main(void) {
  mapping = calloc(256, 4096);
  assert(mapping);
  const size_t counts[] = {1, 3, MDBX_AUXILARY_IOV_MAX,
                           MDBX_AUXILARY_IOV_MAX + 1,
                           MDBX_AUXILARY_IOV_MAX * 2 + 1};
  for (size_t i = 0; i < ARRAY_LENGTH(counts); ++i)
    for (unsigned scenario = 0; scenario < 4; ++scenario)
      check(counts[i], scenario);
  free(mapping);
  puts("prefault resident gaps and iovec batch boundaries passed");
  return 0;
}
