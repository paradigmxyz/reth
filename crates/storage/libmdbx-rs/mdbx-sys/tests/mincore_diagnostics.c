/* Linux: cc -O1 -pthread tests/mincore_diagnostics.c -o /tmp/mincore_diagnostics_test */
#define MDBX_MINCORE_DIAGNOSTICS 1
#define MDBX_ENABLE_PGOP_STAT 1
#define MDBX_BUILD_FLAGS "standalone mincore diagnostics regression test"
#include "../libmdbx/mdbx.c"
#include <assert.h>

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
  mincore_clean_cache(env);
  mincore_diagnostics_t *d = env->mincore_diagnostics;

  /* Untouched anonymous pages are nonresident. The cache optimistically marks
   * a probed bit resident even though this test does not perform a write. */
  assert(!mincore_probe(env, 0));
  assert(mincore_probe(env, 0));
  assert(!mincore_probe(env, 1));
  assert(mincore_probe(env, 1));
  assert(d->probes == 4 && d->hits[0] == 3 && d->misses == 1);
  assert(d->resident_hits == 2 && d->nonresident_hits == 1 && d->nonresident_misses == 1);
  assert(d->repeated_hits == 2 && d->windows[0].used == 3);
  assert(d->queried_os_pages == 64 && d->resident_os_pages == 0 && d->none_resident == 1);

  /* Promotion keeps the usage bitmap attached to the matching cache entry. */
  assert(!mincore_probe(env, 64));
  assert(!mincore_probe(env, 128));
  assert(!mincore_probe(env, 192));
  assert(mincore_probe(env, 64));
  assert(d->hits[2] == 1 && d->windows[0].begin == 64 && d->windows[3].used == 3);
  assert(!mincore_probe(env, 256));
  assert(d->evictions == 1 && d->completed_windows == 1 && d->usage_hist[2] == 1);
  assert(!mincore_probe(env, 0));
  assert(d->ghost_hits[0] == 1);

  /* Explicit invalidation closes live windows and forgets stale ghost ranges. */
  mincore_clean_cache(env);
  assert(d->invalidated_windows == 4 && d->completed_windows == 6);
  assert(d->used_units == 7 && d->completed_units == 384);
  for (size_t i = 0; i < 16; ++i)
    assert(!d->ghosts[i].units);
  assert(!mincore_probe(env, 318));
  assert(d->clipped_queries == 1 && d->windows[0].units == 2);

  /* Residency classification and grouping when one DB page spans two OS pages. */
  mincore_clean_cache(env);
  memset(env->dxb_mmap.base, 0, length);
  env->ps *= 2;
  ++env->ps2ln;
  assert(mincore_probe(env, 0));
  assert(d->all_resident == 1 && d->resident_os_pages == 128 && d->resident_misses == 1);
  assert(d->windows[0].units == 64);
  mincore_clean_cache(env);
  assert(madvise(env->dxb_mmap.base, globals.sys_pagesize, MADV_DONTNEED) == 0);
  assert(!mincore_probe(env, 0));
  assert(d->mixed == 1 && d->resident_os_pages == 255);
  const uint64_t completed = d->completed_windows;
  mincore_diag_report(d, 0, 0, env->ps, globals.sys_pagesize);
  assert(d->completed_windows == completed && d->windows[0].units == 64);

  /* Failed syscalls count as misses/errors, not successful query windows. */
  mincore_clean_cache(env);
  assert(munmap(env->dxb_mmap.base, length) == 0);
  const uint64_t queried = d->queried_os_pages;
  assert(!mincore_probe(env, 0));
  assert(d->errors == 1 && d->queried_os_pages == queried && !d->windows[0].units);
  assert(d->probes == d->misses + d->hits[0] + d->hits[1] + d->hits[2] + d->hits[3]);
  assert(d->misses == d->errors + d->resident_misses + d->nonresident_misses);
  assert(d->untracked_hits == 0);
  mincore_diag_report(d, 1, 0, env->ps, globals.sys_pagesize);
  env->dxb_mmap.base = NULL;
  env->dxb_mmap.current = 0;
  env->lck = NULL;
  mdbx_env_close(env);
  puts("PASS: hit positions, repeated probes, utilization, eviction, clear, tail, page-size grouping, errors");
}
