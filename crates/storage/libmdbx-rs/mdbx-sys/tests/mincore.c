/* SPDX-License-Identifier: Apache-2.0
 * Compile from mdbx-sys with:
 * cc -std=c11 -O1 -g -pthread tests/mincore.c -o /tmp/mdbx-mincore-test
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

#ifndef MINCORE_CACHE_UNITS
#define MINCORE_CACHE_UNITS 64
#endif

static size_t calls, queried_pages;
static size_t expected_offset, expected_length;
static unsigned char residency[1024];
static bool fail_query;
static void *mapping;

static int test_mincore(void *address, size_t length, void *vector) {
  assert((char *)address - (char *)mapping == (ptrdiff_t)expected_offset);
  assert(length == expected_length);
  ++calls;
  queried_pages += length / globals.sys_pagesize;
  if (fail_query) {
    errno = ENOMEM;
    return -1;
  }
  memcpy(vector, residency, length / globals.sys_pagesize);
  return 0;
}

static void reset(MDBX_env *env, lck_t *lck, unsigned db_log, unsigned os_log) {
  memset(env, 0, sizeof(*env));
  memset(lck, 0, sizeof(*lck));
  memset(lck->mincore_cache.begin, -1, sizeof(lck->mincore_cache.begin));
  globals.sys_pagesize_ln2 = os_log;
  globals.sys_pagesize = (size_t)1 << os_log;
  env->ps2ln = db_log;
  env->ps = (size_t)1 << db_log;
  env->lck = lck;
  env->dxb_mmap.base = mapping;
  env->dxb_mmap.current = (size_t)1024 << 16;
  calls = queried_pages = 0;
  fail_query = false;
  memset(residency, 1, sizeof(residency));
}

int main(void) {
  MDBX_env env;
  lck_t lck;
  mapping = malloc((size_t)1024 << 16);
  assert(mapping);
  /* Exercise DB pages smaller than, equal to, and larger than OS pages. */
  for (unsigned db_log = 12; db_log <= 16; db_log += 2) {
    for (unsigned os_log = 12; os_log <= 16; os_log += 2) {
      reset(&env, &lck, db_log, os_log);
      const unsigned unit_log = db_log > os_log ? db_log : os_log;
      const size_t unit = (size_t)1 << unit_log;
      expected_offset = unit * 4;
      expected_length = unit * MINCORE_CACHE_UNITS;
      const pgno_t pgno = (pgno_t)(expected_offset >> db_log);
      /* Any nonresident OS subpage makes the entire DB page nonresident. */
      residency[(unit >> os_log) - 1] = 0;
      residency[(MINCORE_CACHE_UNITS * unit >> os_log) - 1] = 0;
      assert(!mincore_probe(&env, pgno));
      assert(calls == 1);
      assert(mincore_probe(&env, pgno));
      assert(calls == 1);
      assert(mincore_probe(&env, (pgno_t)(5 * unit >> db_log)));
      assert(calls == 1);

      assert(!mincore_probe(
          &env, (pgno_t)((4 + MINCORE_CACHE_UNITS - 1) * unit >> db_log)));
      assert(calls == 1);

      reset(&env, &lck, db_log, os_log);
      expected_offset = unit * 4;
      env.dxb_mmap.current = expected_offset + 3 * unit;
      expected_length = 3 * unit;
      assert(mincore_probe(&env, pgno));
      assert(queried_pages == 3 * unit >> os_log);

      reset(&env, &lck, db_log, os_log);
      expected_length = unit * MINCORE_CACHE_UNITS;
      fail_query = true;
      assert(!mincore_probe(&env, pgno));
      assert(!mincore_probe(&env, pgno));
      assert(calls == 2);
      assert(lck.mincore_cache.begin[0] == P_INVALID);
    }
  }
  /* Four disjoint windows must survive insertion and move-to-front hits. */
  reset(&env, &lck, 12, 12);
  expected_length = (size_t)MINCORE_CACHE_UNITS << 12;
  for (pgno_t n = 1; n <= 4; ++n) {
    expected_offset = (size_t)n * 128 << 12;
    assert(mincore_probe(&env, n * 128));
  }
  assert(calls == 4);
  for (pgno_t n = 1; n <= 4; ++n) {
    assert(mincore_probe(&env, n * 128));
    assert(calls == 4);
  }
  /* The unit immediately outside a window requires a fresh query. */
  expected_offset = (size_t)(4 * 128 + MINCORE_CACHE_UNITS) << 12;
  assert(mincore_probe(&env, 4 * 128 + MINCORE_CACHE_UNITS));
  assert(calls == 5);
  free(mapping);
  puts("mincore residency, page-size, map-tail, and retry tests passed");
  return 0;
}
