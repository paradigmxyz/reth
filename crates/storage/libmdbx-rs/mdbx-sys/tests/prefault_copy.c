/* Run with ./crates/storage/libmdbx-rs/mdbx-sys/tests/prefault_copy.sh.
 * Including the amalgamation gives this regression test access to allocation
 * bookkeeping without exposing test hooks in the library API. */
#define _GNU_SOURCE 1
#define MDBX_BUILD_FLAGS "prefault-copy regression"
#ifndef TEST_UNCONDITIONAL_COW
#define TEST_UNCONDITIONAL_COW 1
#endif
#include <sys/types.h>
#include <unistd.h>
static ssize_t test_pwrite(int fd, const void *buf, size_t count, off_t offset);
#define pwrite test_pwrite
#include "../libmdbx/mdbx.c"
#undef pwrite

static int watched_fd = -1;
static const void *watched_source;
static size_t watched_size;
static unsigned source_calls, total_calls;
static enum { WRITE_OK, WRITE_ERROR, WRITE_PARTIAL_ERROR } write_mode;

static ssize_t test_pwrite(int fd, const void *buf, size_t count, off_t offset) {
  if (fd == watched_fd) {
    ++total_calls;
    if ((uintptr_t)buf >= (uintptr_t)watched_source && (uintptr_t)buf < (uintptr_t)watched_source + watched_size) {
      ++source_calls;
      if (write_mode == WRITE_ERROR || (write_mode == WRITE_PARTIAL_ERROR && source_calls > 1)) {
        errno = EIO;
        return -1;
      }
      if (write_mode == WRITE_PARTIAL_ERROR)
        count /= 2;
    }
  }
  return pwrite(fd, buf, count, offset);
}

#define CHECK(expr)                                                                                                    \
  do {                                                                                                                 \
    if (!(expr)) {                                                                                                     \
      fprintf(stderr, "%s:%d: %s\n", __FILE__, __LINE__, #expr);                                                       \
      abort();                                                                                                         \
    }                                                                                                                  \
  } while (0)
#define OK(expr) CHECK((expr) == MDBX_SUCCESS)

static void check_copy(unsigned pagesize, unsigned flags, unsigned scenario) {
  char path[] = "/tmp/mdbx-prefault-copy-XXXXXX";
  CHECK(mkdtemp(path));
  MDBX_env *env;
  OK(mdbx_env_create(&env));
  OK(mdbx_env_set_geometry(env, 0, 8 << 20, 8 << 20, 0, 0, pagesize));
  OK(mdbx_env_open(env, path, MDBX_WRITEMAP | MDBX_NOSTICKYTHREADS, 0600));
  MDBX_txn *txn;
  OK(mdbx_txn_begin(env, NULL, 0, &txn));
  MDBX_cursor *mc;
  OK(mdbx_cursor_open(txn, MAIN_DBI, &mc));
  txn->flags |= MDBX_TXN_DIRTY;
  const pgno_t source_pgno = txn->geo.first_unallocated++;
  const pgno_t dest_pgno = txn->geo.first_unallocated++;
  page_t *source = pgno2page(env, source_pgno);
  memset(source, 0x2a, pagesize);
  source->pgno = source_pgno;
  source->txnid = txn->txnid - 1;
  source->flags = (uint16_t)flags;
  source->dupfix_ksize = 8;
  source->lower = 8;
  source->upper = (indx_t)(pagesize - PAGEHDRSZ - 64);
  void *snapshot = malloc(pagesize);
  page_t *expected = calloc(1, pagesize), *actual = calloc(1, pagesize);
  CHECK(snapshot && expected && actual);
  memcpy(snapshot, source, pagesize);
  page_copy(expected, source, pagesize);
  expected->pgno = dest_pgno;
  expected->txnid = txn->front_txnid;

  const bool resident = scenario == 3;
  const bool perturb = scenario == 4;
  const bool enabled = scenario != 5;
  txn->tw.prefault_write_activated = enabled;
  if (perturb)
    env->flags |= MDBX_PAGEPERTURB;
#if MDBX_USE_MINCORE
  mincore_clean_cache(env);
  const unsigned unit_log2 = env->ps2ln > globals.sys_pagesize_ln2 ? env->ps2ln : globals.sys_pagesize_ln2;
  env->lck->mincore_cache.begin[0] = (pgno_t)(pgno2bytes(env, dest_pgno) >> unit_log2);
  env->lck->mincore_cache.mask[0] = resident ? UINT64_MAX : 0;
#endif
  watched_fd = env->lazy_fd;
  watched_source = source;
  watched_size = pagesize;
  total_calls = source_calls = 0;
  write_mode = scenario == 1 ? WRITE_ERROR : scenario == 2 ? WRITE_PARTIAL_ERROR : WRITE_OK;
  const pgr_t result = page_alloc_finalize(env, txn, mc, dest_pgno, 1, source);
  watched_fd = -1;
  OK(result.err);
  page_copy(actual, result.page, pagesize);
  CHECK(memcmp(expected, actual, pagesize) == 0);
  CHECK(memcmp(snapshot, source, pagesize) == 0);

  bool eligible = false;
#if defined(__linux__) && !MDBX_MMAP_INCOHERENT_CPU_CACHE && !MDBX_MMAP_INCOHERENT_FILE_WRITE
  eligible = pagesize >= globals.sys_pagesize && !perturb;
#endif
#if !MDBX_USE_MINCORE
  const bool probed_resident = false;
#else
  const bool probed_resident = resident;
#endif
  const bool prefaulted = enabled && (!probed_resident || (TEST_UNCONDITIONAL_COW && eligible));
  const unsigned expected_source_calls = prefaulted && eligible ? (scenario == 2 ? 2 : 1) : 0;
  CHECK(source_calls == expected_source_calls);
  CHECK(total_calls == (prefaulted ? (expected_source_calls ? expected_source_calls : 1) : 0));
  if (perturb)
    CHECK(*((unsigned char *)result.page + PAGEHDRSZ + 128) == 0xff);
  if (source_calls && write_mode == WRITE_OK)
    CHECK(*((unsigned char *)result.page + PAGEHDRSZ + 128) == 0x2a);

  free(snapshot);
  free(expected);
  free(actual);
  mdbx_cursor_close(mc);
  OK(mdbx_txn_abort(txn));
  OK(mdbx_env_close_ex(env, false));
  OK(mdbx_env_delete(path, MDBX_ENV_JUST_DELETE));
}

int main(void) {
  const unsigned sizes[] = {512, 4096, 16384, 65536};
  const unsigned flags[] = {P_BRANCH, P_LEAF, P_LEAF | P_DUPFIX};
  for (size_t s = 0; s < sizeof(sizes) / sizeof(sizes[0]); ++s)
    for (size_t f = 0; f < sizeof(flags) / sizeof(flags[0]); ++f)
      for (unsigned scenario = 0; scenario < 6; ++scenario)
        check_copy(sizes[s], flags[f], scenario);
  puts("prefault copy: 72 page-size/type/failure/residency cases passed");
  return 0;
}
