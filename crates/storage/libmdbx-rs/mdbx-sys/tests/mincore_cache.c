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

  assert(munmap(env->dxb_mmap.base, length) == 0);
  env->dxb_mmap.base = NULL;
  env->dxb_mmap.current = 0;
  env->lck = NULL;
  mdbx_env_close(env);
  puts("PASS: four-window reuse, promotion, and least-recently-used eviction");
  return 0;
}
