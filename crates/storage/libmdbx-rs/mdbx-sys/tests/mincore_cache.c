/* Standalone Linux regression test for the internal residency cache.
 * Run from mdbx-sys:
 * cc -O1 -pthread tests/mincore_cache.c -o /tmp/mincore_cache_test
 * /tmp/mincore_cache_test
 */
#define MDBX_ENABLE_PGOP_STAT 1
#define MDBX_BUILD_FLAGS "standalone mincore cache regression test"
#include "../libmdbx/mdbx.c"
#include <assert.h>
#include <stdio.h>

int main(void) {
  MDBX_env *env = NULL;
  assert(mdbx_env_create(&env) == MDBX_SUCCESS);
  lck_t lock = {0};
  env->lck = &lock;
  env->ps = globals.sys_pagesize;
  env->ps2ln = globals.sys_pagesize_ln2;
  const size_t length = 320 * env->ps;
  env->dxb_mmap.base = mmap(NULL, length, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  assert(env->dxb_mmap.base != MAP_FAILED);
  env->dxb_mmap.current = length;
  memset(env->dxb_mmap.base, 0, length);
  mincore_clean_cache(env);

  /* Four disjoint windows fit. Revisiting each must avoid another syscall. */
  for (pgno_t page = 0; page < 256; page += 64)
    assert(mincore_probe(env, page));
  assert(lock.pgops.mincore.weak == 4);
  const pgno_t revisit[] = {64, 128, 0, 192};
  for (size_t i = 0; i < ARRAY_LENGTH(revisit); ++i) {
    assert(mincore_probe(env, revisit[i]));
    assert(lock.pgops.mincore.weak == 4);
  }

  /* The fifth window evicts page 64, the least recently used window. */
  assert(mincore_probe(env, 256));
  assert(lock.pgops.mincore.weak == 5);
  assert(mincore_probe(env, 128));
  assert(mincore_probe(env, 0));
  assert(mincore_probe(env, 192));
  assert(lock.pgops.mincore.weak == 5);
  assert(mincore_probe(env, 64));
  assert(lock.pgops.mincore.weak == 6);

  /* Both the newest and older entries must reject offset 32, while 31 hits. */
  mincore_clean_cache(env);
  lock.pgops.mincore.weak = 0;
  assert(mincore_probe(env, 0));
  assert(mincore_probe(env, 31));
  assert(lock.pgops.mincore.weak == 1);
  assert(mincore_probe(env, 96));
  assert(mincore_probe(env, 32));
  assert(lock.pgops.mincore.weak == 3);
  assert(mincore_probe(env, 63));
  assert(mincore_probe(env, 31));
  assert(lock.pgops.mincore.weak == 3);
  assert(mincore_probe(env, 64));
  assert(lock.pgops.mincore.weak == 4);

  /* A query at the mapping tail is clipped to one page. */
  mincore_clean_cache(env);
  assert(mincore_probe(env, 319));

  /* Mixed residency and the optimistic bit update still work. */
  assert(madvise(env->dxb_mmap.base, length, MADV_DONTNEED) == 0);
  ((volatile char *)env->dxb_mmap.base)[31 * env->ps] = 1;
  mincore_clean_cache(env);
  lock.pgops.mincore.weak = 0;
  assert(!mincore_probe(env, 0));
  assert(mincore_probe(env, 0));
  assert(mincore_probe(env, 31));
  assert(!mincore_probe(env, 32));
  assert(lock.pgops.mincore.weak == 2);

  /* A DB page spanning two OS pages is resident only if both are resident. */
  assert(madvise(env->dxb_mmap.base, length, MADV_DONTNEED) == 0);
  env->ps *= 2;
  env->ps2ln += 1;
  ((volatile char *)env->dxb_mmap.base)[30 * env->ps] = 1;
  ((volatile char *)env->dxb_mmap.base)[31 * env->ps] = 1;
  ((volatile char *)env->dxb_mmap.base)[31 * env->ps + globals.sys_pagesize] = 1;
  mincore_clean_cache(env);
  lock.pgops.mincore.weak = 0;
  assert(!mincore_probe(env, 0));
  assert(!mincore_probe(env, 30));
  assert(mincore_probe(env, 31));
  assert(!mincore_probe(env, 32));
  assert(lock.pgops.mincore.weak == 2);

  assert(munmap(env->dxb_mmap.base, length) == 0);
  env->dxb_mmap.base = NULL;
  env->dxb_mmap.current = 0;
  env->lck = NULL;
  mdbx_env_close(env);
  puts("PASS: four-window LRU, 32-unit boundaries, tail, mixed residency, and larger DB pages");
  return 0;
}
