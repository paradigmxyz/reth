/* SPDX-License-Identifier: Apache-2.0 */
/*
 * Focused smoke coverage for storage-backend migration work.
 *
 * This test intentionally uses only the public C API while exercising behavior
 * that a no-data-mmap/page-cache backend must preserve: stable read snapshots,
 * returned-value lifetime within a read transaction, large/overflow values,
 * dirty-page spilling in file-write mode, nested transactions, resize,
 * GC/reuse cycles, multiprocess reader/writer MVCC coordination with
 * writer-side geometry growth while a read transaction is pinned, default and
 * forced tiny-cache explicit backend compatibility, lck-less read-only opens,
 * MDBX_WRITEMAP rejection, and process-death/restart boundaries around
 * committed and abandoned writers.
 */

#include "mdbx.h"

#if !defined(_WIN32) && !defined(_WIN64)
#include <errno.h>
#include <pthread.h>
#include <signal.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>
#endif

#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define ARRAY_LENGTH(array) (sizeof(array) / sizeof((array)[0]))
#define MDBX_TEST_LEGACY_MAPASYNC UINT32_C(0x100000)

#if defined(_WIN32) || defined(_WIN64)
#define MDBX_TEST_ENOSPC ERROR_DISK_FULL
#else
#define MDBX_TEST_ENOSPC ENOSPC
#endif

static unsigned long smoke_run_id(void) {
#if !defined(_WIN32) && !defined(_WIN64)
  return (unsigned long)getpid();
#else
  return (unsigned long)(uintptr_t)&smoke_run_id;
#endif
}

static uint64_t round_up_u64(uint64_t value, uint64_t unit) {
  if (unit <= 1)
    return value;
  const uint64_t remainder = value % unit;
  return remainder ? value + unit - remainder : value;
}

static int fail_rc(const char *expr, int rc, const char *file, int line) {
  fprintf(stderr, "%s:%d: %s failed: (%d) %s\n", file, line, expr, rc, mdbx_strerror(rc));
  return rc ? rc : MDBX_PROBLEM;
}

#if defined(MIGRATION_FAULT_INJECTION)
static int expect_rc_value(const char *expr, int rc, int expected, const char *file, int line) {
  if (rc == expected)
    return MDBX_SUCCESS;

  fprintf(stderr, "%s:%d: %s returned (%d) %s, expected (%d) %s\n", file, line, expr, rc, mdbx_strerror(rc),
          expected, mdbx_strerror(expected));
  return rc ? rc : MDBX_PROBLEM;
}
#endif /* MIGRATION_FAULT_INJECTION */

static int fail_msg(const char *msg, const char *file, int line) {
  fprintf(stderr, "%s:%d: %s\n", file, line, msg);
  return MDBX_PROBLEM;
}

#if !defined(_WIN32) && !defined(_WIN64)
static int fail_errno(const char *expr, int errnum, const char *file, int line) {
  fprintf(stderr, "%s:%d: %s failed: errno %d (%s)\n", file, line, expr, errnum, strerror(errnum));
  return MDBX_PROBLEM;
}
#endif

static char *duplicate_cstr(const char *value) {
  const size_t bytes = strlen(value) + 1;
  char *copy = (char *)malloc(bytes);
  if (copy)
    memcpy(copy, value, bytes);
  return copy;
}

static int set_env_var(const char *name, const char *value) {
#if defined(_WIN32) || defined(_WIN64)
  return _putenv_s(name, value) == 0 ? MDBX_SUCCESS : fail_msg("_putenv_s failed", __FILE__, __LINE__);
#else
  return setenv(name, value, 1) == 0 ? MDBX_SUCCESS : fail_errno("setenv", errno, __FILE__, __LINE__);
#endif
}

static int unset_env_var(const char *name) {
#if defined(_WIN32) || defined(_WIN64)
  return _putenv_s(name, "") == 0 ? MDBX_SUCCESS : fail_msg("_putenv_s failed", __FILE__, __LINE__);
#else
  return unsetenv(name) == 0 ? MDBX_SUCCESS : fail_errno("unsetenv", errno, __FILE__, __LINE__);
#endif
}

struct saved_env_var {
  const char *name;
  char *value;
  int present;
};

static int save_env_var(struct saved_env_var *saved, const char *name) {
  const char *value = getenv(name);
  saved->name = name;
  saved->present = value != NULL;
  saved->value = NULL;
  if (value) {
    saved->value = duplicate_cstr(value);
    if (!saved->value)
      return fail_msg("malloc failed", __FILE__, __LINE__);
  }
  return MDBX_SUCCESS;
}

static int restore_env_var(struct saved_env_var *saved) {
  const int rc = saved->present ? set_env_var(saved->name, saved->value) : unset_env_var(saved->name);
  free(saved->value);
  saved->value = NULL;
  saved->present = 0;
  return rc;
}

#define CHECK(expr)                                                                                                    \
  do {                                                                                                                 \
    rc = (expr);                                                                                                       \
    if (rc != MDBX_SUCCESS)                                                                                            \
      goto bailout_rc;                                                                                                 \
  } while (0)

#if defined(MIGRATION_FAULT_INJECTION)
#define EXPECT_RC(expr, expected)                                                                                      \
  do {                                                                                                                 \
    rc = expect_rc_value(#expr, (expr), (expected), __FILE__, __LINE__);                                                \
    if (rc != MDBX_SUCCESS)                                                                                            \
      goto bailout;                                                                                                    \
  } while (0)
#endif /* MIGRATION_FAULT_INJECTION */

#define REQUIRE(condition, message)                                                                                    \
  do {                                                                                                                 \
    if (!(condition)) {                                                                                                \
      rc = fail_msg((message), __FILE__, __LINE__);                                                                    \
      goto bailout;                                                                                                    \
    }                                                                                                                  \
  } while (0)

static MDBX_val val(void *base, size_t len) {
  MDBX_val result;
  result.iov_base = base;
  result.iov_len = len;
  return result;
}

static size_t value_size(uint64_t key) {
  if (key % 31 == 0)
    return 12000 + (size_t)(key % 7) * 1024;
  return 32 + (size_t)(key % 173);
}

static uint64_t value_seed(uint64_t key, unsigned generation) {
  return key * UINT64_C(0x9E3779B97F4A7C15) + generation * UINT64_C(0xD1B54A32D192ED03);
}

static void fill_pattern(void *ptr, size_t bytes, uint64_t seed) {
  unsigned char *out = (unsigned char *)ptr;
  for (size_t i = 0; i < bytes; ++i) {
    seed ^= seed >> 12;
    seed ^= seed << 25;
    seed ^= seed >> 27;
    out[i] = (unsigned char)((seed * UINT64_C(2685821657736338717) + i) >> 56);
  }
}

static int check_pattern(const void *ptr, size_t bytes, uint64_t seed) {
  const unsigned char *actual = (const unsigned char *)ptr;
  for (size_t i = 0; i < bytes; ++i) {
    seed ^= seed >> 12;
    seed ^= seed << 25;
    seed ^= seed >> 27;
    const unsigned char expected = (unsigned char)((seed * UINT64_C(2685821657736338717) + i) >> 56);
    if (actual[i] != expected)
      return 0;
  }
  return 1;
}

static int put_pattern(MDBX_txn *txn, MDBX_dbi dbi, uint64_t key, unsigned generation, void *scratch) {
  MDBX_val k = val(&key, sizeof(key));
  const size_t bytes = value_size(key);
  MDBX_val data = val(scratch, bytes);
  fill_pattern(scratch, bytes, value_seed(key, generation));
  return mdbx_put(txn, dbi, &k, &data, 0);
}

static int reserve_pattern(MDBX_txn *txn, MDBX_dbi dbi, uint64_t key, unsigned generation) {
  MDBX_val k = val(&key, sizeof(key));
  MDBX_val data = val(NULL, value_size(key));
  int rc = mdbx_put(txn, dbi, &k, &data, MDBX_RESERVE);
  if (rc == MDBX_SUCCESS)
    fill_pattern(data.iov_base, data.iov_len, value_seed(key, generation));
  return rc;
}

static int expect_pattern(MDBX_txn *txn, MDBX_dbi dbi, uint64_t key, unsigned generation) {
  MDBX_val k = val(&key, sizeof(key));
  MDBX_val data = val(NULL, 0);
  int rc = mdbx_get(txn, dbi, &k, &data);
  if (rc != MDBX_SUCCESS)
    return fail_rc("mdbx_get", rc, __FILE__, __LINE__);
  if (data.iov_len != value_size(key))
    return fail_msg("unexpected value size", __FILE__, __LINE__);
  if (!check_pattern(data.iov_base, data.iov_len, value_seed(key, generation)))
    return fail_msg("unexpected value bytes", __FILE__, __LINE__);
  return MDBX_SUCCESS;
}

static int expect_notfound(MDBX_txn *txn, MDBX_dbi dbi, uint64_t key) {
  MDBX_val k = val(&key, sizeof(key));
  MDBX_val data = val(NULL, 0);
  const int rc = mdbx_get(txn, dbi, &k, &data);
  return (rc == MDBX_NOTFOUND) ? MDBX_SUCCESS : fail_rc("mdbx_get expected MDBX_NOTFOUND", rc, __FILE__, __LINE__);
}

static int get_gc_info(MDBX_txn *txn, MDBX_gc_info_t *info) {
  int rc = mdbx_gc_info(txn, info, sizeof(*info), NULL, NULL);
  if (rc == MDBX_NOTFOUND) {
    memset(info, 0, sizeof(*info));
    return MDBX_SUCCESS;
  }
  return rc;
}

static int put_dup_uint64(MDBX_txn *txn, MDBX_dbi dbi, uint64_t key, uint64_t data_value) {
  MDBX_val k = val(&key, sizeof(key));
  MDBX_val data = val(&data_value, sizeof(data_value));
  return mdbx_put(txn, dbi, &k, &data, 0);
}

static int cursor_put_dup_uint64(MDBX_cursor *cursor, uint64_t key_value, uint64_t data_value) {
  MDBX_val key = val(&key_value, sizeof(key_value));
  MDBX_val data = val(&data_value, sizeof(data_value));
  return mdbx_cursor_put(cursor, &key, &data, 0);
}

static int expect_is_dirty(MDBX_txn *txn, const void *ptr, int expected, const char *label) {
  const int rc = mdbx_is_dirty(txn, ptr);
  if (rc == expected)
    return MDBX_SUCCESS;

  fprintf(stderr, "%s:%d: %s dirty-state mismatch: expected %d, got %d (%s)\n", __FILE__, __LINE__, label, expected,
          rc, mdbx_strerror(rc));
  return MDBX_PROBLEM;
}

static int exercise_cursorless_value_lifetime(MDBX_txn *txn, MDBX_dbi dbi) {
  int rc;
  MDBX_cursor *cursor = NULL;
  uint64_t held = 31;
  MDBX_val key = val(&held, sizeof(held));
  MDBX_val data = val(NULL, 0);
  CHECK(mdbx_get(txn, dbi, &key, &data));
  CHECK(expect_is_dirty(txn, data.iov_base, MDBX_RESULT_FALSE, "cursorless read value"));
  REQUIRE(data.iov_len == value_size(held), "unexpected cursorless value size");
  const void *held_ptr = data.iov_base;
  const size_t held_len = data.iov_len;

  for (uint64_t i = 32; i < 48; ++i)
    CHECK(expect_pattern(txn, dbi, i, 0));
  REQUIRE(check_pattern(held_ptr, held_len, value_seed(held, 0)), "cursorless value was not retained across gets");

  MDBX_cache_entry_t entry;
  mdbx_cache_init(&entry);
  MDBX_cache_result_t cached = mdbx_cache_get(txn, dbi, &key, &data, &entry);
  if (cached.errcode != MDBX_SUCCESS)
    return fail_rc("mdbx_cache_get", cached.errcode, __FILE__, __LINE__);
  REQUIRE(cached.status == MDBX_CACHE_REFRESHED, "cache get did not refresh the cache entry");
  CHECK(expect_is_dirty(txn, data.iov_base, MDBX_RESULT_FALSE, "refreshed cache value"));
  REQUIRE(data.iov_len == value_size(held), "unexpected cache value size");
  const void *cached_ptr = data.iov_base;
  const size_t cached_len = data.iov_len;

  for (uint64_t i = 48; i < 64; ++i)
    CHECK(expect_pattern(txn, dbi, i, 0));
  REQUIRE(check_pattern(cached_ptr, cached_len, value_seed(held, 0)), "cache value was not retained across gets");

  cached = mdbx_cache_get_SingleThreaded(txn, dbi, &key, &data, &entry);
  if (cached.errcode != MDBX_SUCCESS)
    return fail_rc("mdbx_cache_get_SingleThreaded", cached.errcode, __FILE__, __LINE__);
  REQUIRE(cached.status == MDBX_CACHE_HIT, "single-thread cache get did not hit the cache entry");
  CHECK(expect_is_dirty(txn, data.iov_base, MDBX_RESULT_FALSE, "cache-hit value"));
  REQUIRE(data.iov_len == value_size(held), "unexpected single-thread cache value size");
  REQUIRE(check_pattern(data.iov_base, data.iov_len, value_seed(held, 0)), "single-thread cache value mismatch");
  const void *cache_hit_ptr = data.iov_base;
  const size_t cache_hit_len = data.iov_len;

  CHECK(mdbx_cursor_open(txn, dbi, &cursor));
  for (uint64_t expected = 0; expected < 900; ++expected) {
    MDBX_val scan_key = val(NULL, 0);
    MDBX_val scan_data = val(NULL, 0);
    CHECK(mdbx_cursor_get(cursor, &scan_key, &scan_data, expected ? MDBX_NEXT : MDBX_FIRST));
    REQUIRE(scan_key.iov_len == sizeof(expected), "cursor scan returned wrong key size");
    uint64_t actual = 0;
    memcpy(&actual, scan_key.iov_base, sizeof(actual));
    REQUIRE(actual == expected, "cursor scan returned unexpected key");
    REQUIRE(scan_data.iov_len == value_size(actual), "cursor scan returned wrong value size");
    REQUIRE(check_pattern(scan_data.iov_base, scan_data.iov_len, value_seed(actual, 0)),
            "cursor scan returned wrong value bytes");
  }
  rc = mdbx_cursor_get(cursor, &key, &data, MDBX_NEXT);
  REQUIRE(rc == MDBX_NOTFOUND, "cursor scan saw unexpected extra item");
  rc = MDBX_SUCCESS;
  mdbx_cursor_close(cursor);
  cursor = NULL;
  REQUIRE(check_pattern(cache_hit_ptr, cache_hit_len, value_seed(held, 0)),
          "cache-hit value was not retained across cursor scan");

  uint64_t lowerbound = 30;
  key = val(&lowerbound, sizeof(lowerbound));
  data = val(NULL, 0);
  rc = mdbx_get_equal_or_great(txn, dbi, &key, &data);
  REQUIRE(rc == MDBX_SUCCESS || rc == MDBX_RESULT_TRUE, "lowerbound get failed");
  REQUIRE(key.iov_len == sizeof(lowerbound), "lowerbound returned wrong key size");
  uint64_t actual;
  memcpy(&actual, key.iov_base, sizeof(actual));
  REQUIRE(actual >= lowerbound, "lowerbound returned smaller key");
  REQUIRE(check_pattern(data.iov_base, data.iov_len, value_seed(actual, 0)), "lowerbound value mismatch");

  size_t values_count = 0;
  key = val(&held, sizeof(held));
  data = val(NULL, 0);
  CHECK(mdbx_get_ex(txn, dbi, &key, &data, &values_count));
  REQUIRE(values_count == 1, "unexpected value count for non-dupsort table");
  REQUIRE(check_pattern(data.iov_base, data.iov_len, value_seed(held, 0)), "mdbx_get_ex value mismatch");

  return MDBX_SUCCESS;

bailout:
  if (cursor)
    mdbx_cursor_close(cursor);
  return rc;

bailout_rc:
  rc = fail_rc("cursorless lifetime operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int count_records(MDBX_txn *txn, MDBX_dbi dbi, size_t *out) {
  int rc;
  MDBX_cursor *cursor = NULL;
  MDBX_val key = val(NULL, 0), data = val(NULL, 0);
  size_t count = 0;

  CHECK(mdbx_cursor_open(txn, dbi, &cursor));
  while ((rc = mdbx_cursor_get(cursor, &key, &data, MDBX_NEXT)) == MDBX_SUCCESS)
    ++count;
  if (rc != MDBX_NOTFOUND)
    goto bailout_rc;

  rc = MDBX_SUCCESS;
  *out = count;

bailout:
  if (cursor)
    mdbx_cursor_close(cursor);
  return rc;

bailout_rc:
  rc = fail_rc("cursor iteration", rc, __FILE__, __LINE__);
  goto bailout;
}

static int check_env_info_snapshot(MDBX_env *env, MDBX_txn *txn) {
  int rc;
  MDBX_envinfo info;
  int recent_seen = 0;

  CHECK(mdbx_env_info_ex(env, txn, &info, sizeof(info)));
  REQUIRE(info.mi_dxb_pagesize >= 256, "unexpected environment page size");
  REQUIRE((info.mi_dxb_pagesize & (info.mi_dxb_pagesize - 1)) == 0, "environment page size is not power-of-two");
  REQUIRE(info.mi_recent_txnid != 0, "missing recent transaction id in env info");
  for (size_t i = 0; i < 3; ++i)
    recent_seen |= (info.mi_meta_txnid[i] == info.mi_recent_txnid);
  REQUIRE(recent_seen, "recent transaction id not present in meta txnids");
  REQUIRE(info.mi_geo.current <= info.mi_mapsize, "current geometry exceeds map size");
  REQUIRE(info.mi_dxb_fsize >= info.mi_dxb_pagesize * 3u, "data file is too small for meta pages");
  REQUIRE(info.mi_last_pgno + 1 <= info.mi_geo.current / info.mi_dxb_pagesize, "last page exceeds current geometry");

  return MDBX_SUCCESS;

bailout:
  return rc;

bailout_rc:
  rc = fail_rc("env info operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int check_txn_observer_apis(MDBX_txn *txn, uint64_t min_reader_lag, int check_refresh_current) {
  int rc;
  MDBX_txn_info info;
  int percent = -1;

  CHECK(mdbx_txn_info(txn, &info, false));
  REQUIRE(info.txn_id == mdbx_txn_id(txn), "transaction info returned wrong txn id");
  REQUIRE(info.txn_space_used <= info.txn_space_limit_soft, "transaction used space exceeds soft limit");
  REQUIRE(info.txn_space_limit_soft <= info.txn_space_limit_hard, "transaction soft limit exceeds hard limit");
  REQUIRE(info.txn_reader_lag >= min_reader_lag, "transaction reader lag is lower than expected");

#if defined(__GNUC__) || defined(__clang__)
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wdeprecated-declarations"
#endif
  const int lag = mdbx_txn_straggler(txn, &percent);
#if defined(__GNUC__) || defined(__clang__)
#pragma GCC diagnostic pop
#endif
  if (lag < 0)
    return fail_rc("mdbx_txn_straggler", lag, __FILE__, __LINE__);
  REQUIRE((uint64_t)lag >= min_reader_lag, "straggler lag is lower than expected");
  REQUIRE(percent >= 0 && percent <= 100, "straggler percent is out of range");

  if (check_refresh_current) {
    rc = mdbx_txn_refresh(txn);
    REQUIRE(rc == MDBX_RESULT_TRUE, "refresh did not report current read snapshot");
  }

  return MDBX_SUCCESS;

bailout:
  return rc;

bailout_rc:
  rc = fail_rc("txn observer operation", rc, __FILE__, __LINE__);
  goto bailout;
}

struct reader_list_state {
  unsigned entries;
  unsigned active;
  uint64_t max_lag;
  size_t max_bytes_used;
};

static int reader_list_count(void *ctx, int num, int slot, mdbx_pid_t pid, mdbx_tid_t thread, uint64_t txnid,
                             uint64_t lag, size_t bytes_used, size_t bytes_retained) {
  (void)num;
  (void)slot;
  (void)pid;
  (void)thread;
  (void)bytes_retained;
  struct reader_list_state *state = (struct reader_list_state *)ctx;
  state->entries += 1;
  if (txnid) {
    state->active += 1;
    if (state->max_lag < lag)
      state->max_lag = lag;
    if (state->max_bytes_used < bytes_used)
      state->max_bytes_used = bytes_used;
  }
  return MDBX_SUCCESS;
}

static int check_reader_list_snapshot(MDBX_env *env, unsigned min_active, uint64_t min_lag) {
  int rc;
  struct reader_list_state state;
  memset(&state, 0, sizeof(state));

  CHECK(mdbx_reader_list(env, reader_list_count, &state));
  REQUIRE(state.entries >= min_active, "reader list returned fewer entries than expected");
  REQUIRE(state.active >= min_active, "reader list returned fewer active readers than expected");
  REQUIRE(state.max_lag >= min_lag, "reader list lag is lower than expected");
  REQUIRE(state.max_bytes_used > 0, "reader list did not report used snapshot bytes");
  return MDBX_SUCCESS;

bailout:
  return rc;

bailout_rc:
  rc = fail_rc("reader list operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int expect_env_sync_ok(MDBX_env *env, bool force, bool nonblock, const char *label) {
  const int rc = mdbx_env_sync_ex(env, force, nonblock);
  if (rc == MDBX_SUCCESS || rc == MDBX_RESULT_TRUE)
    return MDBX_SUCCESS;
  return fail_rc(label, rc, __FILE__, __LINE__);
}

static int exercise_manual_sync(MDBX_env *env, MDBX_dbi items, void *scratch) {
  int rc;
  MDBX_txn *txn = NULL;
  MDBX_envinfo info;
  size_t sync_bytes = 0;
  unsigned sync_period = 0;
  int options_set = 0;

  CHECK(mdbx_env_info_ex(env, NULL, &info, sizeof(info)));
  CHECK(mdbx_env_set_syncbytes(env, 1));
  CHECK(mdbx_env_set_syncperiod(env, 1));
  options_set = 1;
  CHECK(mdbx_env_get_syncbytes(env, &sync_bytes));
  CHECK(mdbx_env_get_syncperiod(env, &sync_period));
  REQUIRE(sync_bytes == info.mi_dxb_pagesize, "sync-bytes threshold did not round to one DB page");
  REQUIRE(sync_period == 1, "sync-period threshold roundtrip failed");

  CHECK(expect_env_sync_ok(env, false, true, "mdbx_env_sync_poll before write"));

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(put_pattern(txn, items, 17, 4, scratch));
  CHECK(put_pattern(txn, items, 31, 4, scratch));
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  CHECK(expect_env_sync_ok(env, false, true, "mdbx_env_sync_poll after write"));
  CHECK(expect_env_sync_ok(env, true, false, "mdbx_env_sync after write"));
  CHECK(expect_env_sync_ok(env, true, true, "nonblocking forced mdbx_env_sync after write"));

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(expect_pattern(txn, items, 17, 4));
  CHECK(expect_pattern(txn, items, 31, 4));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  CHECK(mdbx_env_set_syncbytes(env, 0));
  CHECK(mdbx_env_set_syncperiod(env, 0));
  options_set = 0;
  return MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (options_set) {
    int reset_rc = mdbx_env_set_syncbytes(env, 0);
    if (rc == MDBX_SUCCESS && reset_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_set_syncbytes reset", reset_rc, __FILE__, __LINE__);
    reset_rc = mdbx_env_set_syncperiod(env, 0);
    if (rc == MDBX_SUCCESS && reset_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_set_syncperiod reset", reset_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("manual sync operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int exercise_recovery_open_reject_active(MDBX_env *env, const char *path, MDBX_dbi items, void *scratch) {
  int rc;
  MDBX_txn *txn = NULL;

  rc = mdbx_env_open_for_recovery(env, path, 0, false);
  REQUIRE(rc == MDBX_EPERM, "active environment recovery-open was not rejected");
  rc = MDBX_SUCCESS;

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(put_pattern(txn, items, 31, 0, scratch));
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(expect_pattern(txn, items, 31, 0));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  return MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  return rc;

bailout_rc:
  rc = fail_rc("active recovery-open rejection operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int exercise_active_open_option_rejections(MDBX_env *env) {
  int rc;

  rc = mdbx_env_set_option(env, MDBX_opt_max_db, 8);
  REQUIRE(rc == MDBX_EPERM, "active environment max-db option was not rejected");
  rc = mdbx_env_set_option(env, MDBX_opt_max_readers, 8);
  REQUIRE(rc == MDBX_EPERM, "active environment max-readers option was not rejected");

  return MDBX_SUCCESS;

bailout:
  return rc;
}

static int grow_active_geometry(MDBX_env *env, uint64_t increment, const char *label) {
  int rc;
  MDBX_envinfo before, after;

  CHECK(mdbx_env_info_ex(env, NULL, &before, sizeof(before)));
  REQUIRE(increment > 0, "invalid geometry growth increment");
  REQUIRE(before.mi_geo.current + increment > before.mi_geo.current, "geometry growth overflow");
  const uint64_t requested_now = before.mi_geo.current + increment;
  REQUIRE(requested_now < before.mi_geo.upper, "not enough geometry headroom for growth check");
  CHECK(mdbx_env_set_geometry(env, -1, (intptr_t)requested_now, -1, 128 * 1024, 256 * 1024, -1));
  CHECK(mdbx_env_info_ex(env, NULL, &after, sizeof(after)));
  REQUIRE(after.mi_geo.current >= requested_now, label);
  REQUIRE(after.mi_geo.upper == before.mi_geo.upper, "geometry upper bound changed unexpectedly");

  return MDBX_SUCCESS;

bailout:
  return rc;

bailout_rc:
  rc = fail_rc("geometry growth operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int shrink_active_geometry(MDBX_env *env, uint64_t reserve, const char *label) {
  int rc;
  MDBX_envinfo before, after;

  CHECK(mdbx_env_info_ex(env, NULL, &before, sizeof(before)));
  REQUIRE(before.mi_dxb_pagesize > 0, "missing database page size");
  REQUIRE(before.mi_sys_pagesize > 0, "missing system page size");

  const uint64_t used = (before.mi_last_pgno + 1) * before.mi_dxb_pagesize;
  uint64_t unit = before.mi_dxb_pagesize;
  if (unit < before.mi_sys_pagesize)
    unit = before.mi_sys_pagesize;
  if (unit < before.mi_geo.grow)
    unit = before.mi_geo.grow;
  uint64_t requested_now = round_up_u64(used + reserve, unit);
  if (requested_now < before.mi_geo.lower)
    requested_now = before.mi_geo.lower;

  if (requested_now >= before.mi_geo.current) {
    CHECK(grow_active_geometry(env, 256 * 1024, "geometry current size did not grow before shrink"));
    CHECK(mdbx_env_info_ex(env, NULL, &before, sizeof(before)));
    requested_now = round_up_u64(((before.mi_last_pgno + 1) * before.mi_dxb_pagesize) + reserve, unit);
    if (requested_now < before.mi_geo.lower)
      requested_now = before.mi_geo.lower;
  }
  REQUIRE(requested_now < before.mi_geo.current, "not enough free geometry for shrink check");
  CHECK(mdbx_env_set_geometry(env, -1, (intptr_t)requested_now, -1, (intptr_t)before.mi_geo.grow,
                              (intptr_t)before.mi_geo.shrink, -1));
  CHECK(mdbx_env_info_ex(env, NULL, &after, sizeof(after)));
  REQUIRE(after.mi_geo.current < before.mi_geo.current, label);
  REQUIRE(after.mi_geo.current >= requested_now, "geometry shrank below requested size");
  REQUIRE(after.mi_geo.upper == before.mi_geo.upper, "geometry upper bound changed unexpectedly");
  REQUIRE(after.mi_dxb_fsize <= before.mi_dxb_fsize, "data file grew during shrink check");
  CHECK(check_env_info_snapshot(env, NULL));

  return MDBX_SUCCESS;

bailout:
  return rc;

bailout_rc:
  rc = fail_rc("geometry shrink operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int cursor_expect_current(MDBX_cursor *cursor, uint64_t expected_key, unsigned expected_generation) {
  int rc;
  uint64_t actual_key = 0;
  MDBX_val key = val(NULL, 0), data = val(NULL, 0);

  CHECK(mdbx_cursor_get(cursor, &key, &data, MDBX_GET_CURRENT));
  REQUIRE(key.iov_len == sizeof(actual_key), "cursor current returned wrong key size");
  memcpy(&actual_key, key.iov_base, sizeof(actual_key));
  REQUIRE(actual_key == expected_key, "cursor current returned wrong key");
  REQUIRE(data.iov_len == value_size(expected_key), "cursor current returned wrong value size");
  REQUIRE(check_pattern(data.iov_base, data.iov_len, value_seed(expected_key, expected_generation)),
          "cursor current returned wrong value bytes");
  return MDBX_SUCCESS;

bailout:
  return rc;

bailout_rc:
  rc = fail_rc("cursor current operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int item_pair_expect(MDBX_val *key, MDBX_val *data, uint64_t expected_key, unsigned expected_generation) {
  uint64_t actual_key = 0;
  if (key->iov_len != sizeof(actual_key))
    return fail_msg("cursor returned wrong key size", __FILE__, __LINE__);
  memcpy(&actual_key, key->iov_base, sizeof(actual_key));
  if (actual_key != expected_key)
    return fail_msg("cursor returned wrong key", __FILE__, __LINE__);
  if (data->iov_len != value_size(expected_key))
    return fail_msg("cursor returned wrong value size", __FILE__, __LINE__);
  if (!check_pattern(data->iov_base, data->iov_len, value_seed(expected_key, expected_generation)))
    return fail_msg("cursor returned wrong value bytes", __FILE__, __LINE__);
  return MDBX_SUCCESS;
}

static int cursor_move_item_expect(MDBX_cursor *cursor, MDBX_cursor_op op, uint64_t expected_key,
                                   unsigned expected_generation) {
  int rc;
  MDBX_val key = val(NULL, 0), data = val(NULL, 0);

  CHECK(mdbx_cursor_get(cursor, &key, &data, op));
  CHECK(item_pair_expect(&key, &data, expected_key, expected_generation));
  return MDBX_SUCCESS;

bailout:
  return rc;

bailout_rc:
  rc = fail_rc("cursor movement operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int cursor_set_range_expect(MDBX_cursor *cursor, uint64_t seek_key, uint64_t expected_key,
                                   unsigned expected_generation) {
  int rc;
  MDBX_val key = val(&seek_key, sizeof(seek_key));
  MDBX_val data = val(NULL, 0);

  CHECK(mdbx_cursor_get(cursor, &key, &data, MDBX_SET_RANGE));
  CHECK(item_pair_expect(&key, &data, expected_key, expected_generation));
  return MDBX_SUCCESS;

bailout:
  return rc;

bailout_rc:
  rc = fail_rc("cursor set-range operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int cursor_put_pattern(MDBX_cursor *cursor, uint64_t key_value, unsigned generation, void *scratch,
                              MDBX_put_flags_t flags) {
  MDBX_val key = val(&key_value, sizeof(key_value));
  MDBX_val data = val(scratch, value_size(key_value));
  fill_pattern(scratch, data.iov_len, value_seed(key_value, generation));
  return mdbx_cursor_put(cursor, &key, &data, flags);
}

static int cursor_set_key_expect(MDBX_cursor *cursor, uint64_t expected_key, unsigned expected_generation,
                                 MDBX_val *out_data) {
  int rc;
  MDBX_val key = val(&expected_key, sizeof(expected_key));
  MDBX_val data = val(NULL, 0);

  CHECK(mdbx_cursor_get(cursor, &key, &data, MDBX_SET_KEY));
  REQUIRE(key.iov_len == sizeof(expected_key), "cursor set-key returned wrong key size");
  uint64_t actual_key = 0;
  memcpy(&actual_key, key.iov_base, sizeof(actual_key));
  REQUIRE(actual_key == expected_key, "cursor set-key returned wrong key");
  REQUIRE(data.iov_len == value_size(expected_key), "cursor set-key returned wrong value size");
  REQUIRE(check_pattern(data.iov_base, data.iov_len, value_seed(expected_key, expected_generation)),
          "cursor set-key returned wrong value bytes");
  if (out_data)
    *out_data = data;
  return MDBX_SUCCESS;

bailout:
  return rc;

bailout_rc:
  rc = fail_rc("cursor set-key operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int exercise_cursor_copy_reset_renew(MDBX_env *env, MDBX_dbi dbi, uint64_t first_key, unsigned first_generation,
                                            uint64_t second_key, unsigned second_generation) {
  int rc;
  MDBX_txn *txn = NULL;
  MDBX_cursor *source = NULL, *copy = NULL;
  MDBX_val copied_key = val(NULL, 0);
  MDBX_val copied_data = val(NULL, 0);

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_cursor_open(txn, dbi, &source));
  CHECK(mdbx_cursor_open(txn, dbi, &copy));

  CHECK(cursor_set_key_expect(source, first_key, first_generation, NULL));
  CHECK(mdbx_cursor_copy(source, copy));
  CHECK(cursor_expect_current(copy, first_key, first_generation));
  CHECK(mdbx_cursor_get(copy, &copied_key, &copied_data, MDBX_GET_CURRENT));
  REQUIRE(check_pattern(copied_data.iov_base, copied_data.iov_len, value_seed(first_key, first_generation)),
          "copied cursor value mismatch before source reset");

  CHECK(mdbx_cursor_reset(source));
  CHECK(cursor_set_key_expect(source, second_key, second_generation, NULL));
  CHECK(cursor_expect_current(copy, first_key, first_generation));
  REQUIRE(check_pattern(copied_data.iov_base, copied_data.iov_len, value_seed(first_key, first_generation)),
          "copied cursor value was not retained after source reset/reposition");

  CHECK(mdbx_cursor_reset(copy));
  CHECK(cursor_set_key_expect(copy, second_key, second_generation, NULL));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_cursor_renew(txn, source));
  CHECK(cursor_set_key_expect(source, first_key, first_generation, NULL));
  CHECK(mdbx_cursor_renew(txn, copy));
  CHECK(cursor_set_key_expect(copy, second_key, second_generation, NULL));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (copy)
    mdbx_cursor_close(copy);
  if (source)
    mdbx_cursor_close(source);
  return rc;

bailout_rc:
  rc = fail_rc("cursor copy/reset/renew operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int dup_value_from_val(const MDBX_val *value, uint64_t *out) {
  if (value->iov_len != sizeof(*out))
    return fail_msg("dupsort value returned wrong size", __FILE__, __LINE__);
  memcpy(out, value->iov_base, sizeof(*out));
  return MDBX_SUCCESS;
}

static int dup_expect_pair(MDBX_val *key, MDBX_val *data, uint64_t expected_key, uint64_t expected_data) {
  uint64_t actual_key = 0, actual_data = 0;
  if (key->iov_len != sizeof(actual_key))
    return fail_msg("dupsort cursor returned wrong key size", __FILE__, __LINE__);
  memcpy(&actual_key, key->iov_base, sizeof(actual_key));
  if (actual_key != expected_key)
    return fail_msg("dupsort cursor returned wrong key", __FILE__, __LINE__);
  int rc = dup_value_from_val(data, &actual_data);
  if (rc != MDBX_SUCCESS)
    return rc;
  if (actual_data != expected_data)
    return fail_msg("dupsort cursor returned wrong value", __FILE__, __LINE__);
  return MDBX_SUCCESS;
}

static int dup_move_expect(MDBX_cursor *cursor, MDBX_cursor_op op, uint64_t expected_key, uint64_t expected_data) {
  int rc;
  MDBX_val key = val(NULL, 0), data = val(NULL, 0);

  CHECK(mdbx_cursor_get(cursor, &key, &data, op));
  CHECK(dup_expect_pair(&key, &data, expected_key, expected_data));
  return MDBX_SUCCESS;

bailout:
  return rc;

bailout_rc:
  rc = fail_rc("dupsort cursor move operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int dup_set_key_expect(MDBX_cursor *cursor, uint64_t expected_key, uint64_t expected_data) {
  int rc;
  MDBX_val key = val(&expected_key, sizeof(expected_key));
  MDBX_val data = val(NULL, 0);

  CHECK(mdbx_cursor_get(cursor, &key, &data, MDBX_SET_KEY));
  CHECK(dup_expect_pair(&key, &data, expected_key, expected_data));
  return MDBX_SUCCESS;

bailout:
  return rc;

bailout_rc:
  rc = fail_rc("dupsort set-key operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int dup_get_both_expect(MDBX_cursor *cursor, uint64_t expected_key, uint64_t expected_data, MDBX_cursor_op op) {
  int rc;
  MDBX_val key = val(&expected_key, sizeof(expected_key));
  MDBX_val data = val(&expected_data, sizeof(expected_data));

  CHECK(mdbx_cursor_get(cursor, &key, &data, op));
  CHECK(dup_expect_pair(&key, &data, expected_key, expected_data));
  return MDBX_SUCCESS;

bailout:
  return rc;

bailout_rc:
  rc = fail_rc("dupsort key/value seek operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int populate_dupsort(MDBX_txn *txn, MDBX_dbi dbi) {
  int rc;
  for (uint64_t key = 42; key <= 44; ++key) {
    for (uint64_t i = 0; i < 768; ++i) {
      const uint64_t data_value = 767 - i;
      CHECK(put_dup_uint64(txn, dbi, key, data_value));
    }
  }
  return MDBX_SUCCESS;

bailout:
  return rc;

bailout_rc:
  rc = fail_rc("dupsort populate operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int expect_dup_key_notfound(MDBX_cursor *cursor, uint64_t key_value) {
  MDBX_val key = val(&key_value, sizeof(key_value));
  MDBX_val data = val(NULL, 0);
  const int rc = mdbx_cursor_get(cursor, &key, &data, MDBX_SET_KEY);
  return (rc == MDBX_NOTFOUND) ? MDBX_SUCCESS : fail_rc("dupsort key expected MDBX_NOTFOUND", rc, __FILE__, __LINE__);
}

static int expect_dup_pair_notfound(MDBX_cursor *cursor, uint64_t key_value, uint64_t data_value) {
  MDBX_val key = val(&key_value, sizeof(key_value));
  MDBX_val data = val(&data_value, sizeof(data_value));
  const int rc = mdbx_cursor_get(cursor, &key, &data, MDBX_GET_BOTH);
  return (rc == MDBX_NOTFOUND) ? MDBX_SUCCESS : fail_rc("dupsort pair expected MDBX_NOTFOUND", rc, __FILE__, __LINE__);
}

static int exercise_dupsort_subcursor(MDBX_env *env, MDBX_dbi dbi) {
  int rc;
  MDBX_txn *txn = NULL;
  MDBX_cursor *source = NULL, *copy = NULL;
  size_t duplicates = 0;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_cursor_open(txn, dbi, &source));
  CHECK(mdbx_cursor_open(txn, dbi, &copy));
  CHECK(dup_get_both_expect(source, 42, 123, MDBX_GET_BOTH));
  CHECK(mdbx_cursor_count(source, &duplicates));
  REQUIRE(duplicates == 768, "unexpected initial duplicate count");

  CHECK(mdbx_cursor_copy(source, copy));
  CHECK(dup_move_expect(copy, MDBX_GET_CURRENT, 42, 123));
  MDBX_val held_key = val(NULL, 0), held_data = val(NULL, 0);
  CHECK(mdbx_cursor_get(copy, &held_key, &held_data, MDBX_GET_CURRENT));
  CHECK(dup_expect_pair(&held_key, &held_data, 42, 123));
  const void *held_ptr = held_data.iov_base;
  const size_t held_len = held_data.iov_len;

  CHECK(dup_move_expect(source, MDBX_LAST_DUP, 42, 767));
  CHECK(dup_move_expect(source, MDBX_PREV_DUP, 42, 766));
  CHECK(dup_get_both_expect(source, 42, 125, MDBX_GET_BOTH_RANGE));
  CHECK(dup_move_expect(source, MDBX_NEXT_DUP, 42, 126));
  CHECK(dup_move_expect(source, MDBX_NEXT_NODUP, 43, 0));
  CHECK(dup_move_expect(copy, MDBX_GET_CURRENT, 42, 123));
  {
    uint64_t held_value = 0;
    MDBX_val held = val((void *)held_ptr, held_len);
    CHECK(dup_value_from_val(&held, &held_value));
    REQUIRE(held_value == 123, "copied dupsort cursor value was not retained");
  }
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  mdbx_cursor_close(copy);
  copy = NULL;
  mdbx_cursor_close(source);
  source = NULL;

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_cursor_open(txn, dbi, &source));
  CHECK(dup_get_both_expect(source, 42, 10, MDBX_GET_BOTH));
  CHECK(mdbx_cursor_del(source, MDBX_CURRENT));
  CHECK(expect_dup_pair_notfound(source, 42, 10));
  CHECK(dup_get_both_expect(source, 42, 11, MDBX_GET_BOTH));
  CHECK(mdbx_cursor_count(source, &duplicates));
  REQUIRE(duplicates == 767, "unexpected duplicate count after deleting one duplicate");
  CHECK(dup_set_key_expect(source, 43, 0));
  CHECK(mdbx_cursor_del(source, MDBX_ALLDUPS));
  mdbx_cursor_close(source);
  source = NULL;
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_cursor_open(txn, dbi, &source));
  CHECK(dup_get_both_expect(source, 42, 11, MDBX_GET_BOTH));
  CHECK(mdbx_cursor_count(source, &duplicates));
  REQUIRE(duplicates == 767, "unexpected duplicate count after commit");
  CHECK(expect_dup_key_notfound(source, 43));
  CHECK(dup_set_key_expect(source, 44, 0));
  CHECK(mdbx_cursor_count(source, &duplicates));
  REQUIRE(duplicates == 768, "unexpected duplicate count for untouched key");
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (copy)
    mdbx_cursor_close(copy);
  if (source)
    mdbx_cursor_close(source);
  if (txn)
    mdbx_txn_abort(txn);
  return rc;

bailout_rc:
  rc = fail_rc("dupsort subcursor operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static unsigned cursor_churn_generation(uint64_t key) { return key == 17 ? 1u : 0u; }

static int exercise_cursor_invalidation_churn(MDBX_env *env, MDBX_dbi items, MDBX_dbi dups) {
  static const uint64_t overflow_keys[] = {31,  62,  93,  124, 155, 186, 217, 248, 279, 310,
                                           341, 372, 403, 434, 465, 496, 527, 558, 589, 620,
                                           651, 682, 713, 744, 775, 806, 837, 868, 5000, 17};
  int rc;
  MDBX_txn *txn = NULL;
  MDBX_cursor *item_cursor = NULL, *dup_cursor = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_cursor_open(txn, items, &item_cursor));
  CHECK(mdbx_cursor_open(txn, dups, &dup_cursor));

  for (size_t i = 0; i < ARRAY_LENGTH(overflow_keys); ++i)
    CHECK(cursor_set_key_expect(item_cursor, overflow_keys[i], cursor_churn_generation(overflow_keys[i]), NULL));

  CHECK(cursor_move_item_expect(item_cursor, MDBX_FIRST, 0, 0));
  for (uint64_t expected = 1; expected < 130; ++expected) {
    if (expected == 23)
      continue;
    CHECK(cursor_move_item_expect(item_cursor, MDBX_NEXT, expected, cursor_churn_generation(expected)));
  }

  CHECK(cursor_set_range_expect(item_cursor, 23, 24, 0));
  CHECK(cursor_set_range_expect(item_cursor, 900, 5000, 0));
  CHECK(cursor_move_item_expect(item_cursor, MDBX_LAST, 5000, 0));
  for (uint64_t expected = 899; expected > 820; --expected)
    CHECK(cursor_move_item_expect(item_cursor, MDBX_PREV, expected, 0));

  CHECK(dup_get_both_expect(dup_cursor, 42, 11, MDBX_GET_BOTH));
  for (uint64_t expected = 12; expected < 300; ++expected)
    CHECK(dup_move_expect(dup_cursor, MDBX_NEXT_DUP, 42, expected));
  CHECK(dup_move_expect(dup_cursor, MDBX_LAST_DUP, 42, 767));
  for (uint64_t expected = 766; expected > 700; --expected)
    CHECK(dup_move_expect(dup_cursor, MDBX_PREV_DUP, 42, expected));
  CHECK(dup_move_expect(dup_cursor, MDBX_NEXT_NODUP, 44, 0));
  CHECK(dup_move_expect(dup_cursor, MDBX_LAST_DUP, 44, 767));
  for (uint64_t expected = 766; expected > 720; --expected)
    CHECK(dup_move_expect(dup_cursor, MDBX_PREV_DUP, 44, expected));
  CHECK(expect_dup_key_notfound(dup_cursor, 43));

  CHECK(mdbx_txn_abort(txn));
  txn = NULL;
  mdbx_cursor_close(dup_cursor);
  dup_cursor = NULL;
  mdbx_cursor_close(item_cursor);
  item_cursor = NULL;
  return MDBX_SUCCESS;

bailout:
  if (dup_cursor)
    mdbx_cursor_close(dup_cursor);
  if (item_cursor)
    mdbx_cursor_close(item_cursor);
  if (txn)
    mdbx_txn_abort(txn);
  return rc;

bailout_rc:
  rc = fail_rc("cursor invalidation churn operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int exercise_write_cursor_invalidation_churn(MDBX_env *env, MDBX_dbi items, MDBX_dbi dups, void *scratch) {
  int rc;
  MDBX_txn *txn = NULL;
  MDBX_cursor *item_cursor = NULL, *item_check = NULL, *dup_cursor = NULL, *dup_check = NULL;
  size_t duplicates = 0;

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_cursor_open(txn, items, &item_cursor));
  CHECK(mdbx_cursor_open(txn, items, &item_check));
  CHECK(mdbx_cursor_open(txn, dups, &dup_cursor));
  CHECK(mdbx_cursor_open(txn, dups, &dup_check));

  CHECK(cursor_set_key_expect(item_cursor, 31, 0, NULL));
  CHECK(cursor_put_pattern(item_cursor, 31, 11, scratch, MDBX_CURRENT));
  {
    MDBX_val dirty_data = val(NULL, 0);
    CHECK(cursor_set_key_expect(item_cursor, 31, 11, &dirty_data));
    CHECK(expect_is_dirty(txn, dirty_data.iov_base, MDBX_RESULT_TRUE, "write cursor invalidation updated value"));
  }
  CHECK(cursor_move_item_expect(item_cursor, MDBX_NEXT, 32, 0));
  CHECK(cursor_set_key_expect(item_check, 31, 11, NULL));

  CHECK(cursor_set_range_expect(item_cursor, 120, 120, 0));
  for (uint64_t key = 120; key <= 260; ++key) {
    CHECK(mdbx_cursor_del(item_cursor, MDBX_CURRENT));
    if (key < 260)
      CHECK(cursor_expect_current(item_cursor, key + 1, 0));
  }
  CHECK(cursor_set_range_expect(item_check, 120, 261, 0));
  for (uint64_t key = 120; key <= 260; ++key) {
    CHECK(cursor_put_pattern(item_cursor, key, 12, scratch, MDBX_NOOVERWRITE));
    CHECK(cursor_set_key_expect(item_cursor, key, 12, NULL));
  }
  CHECK(cursor_set_range_expect(item_cursor, 120, 120, 12));
  for (uint64_t key = 121; key <= 260; ++key)
    CHECK(cursor_move_item_expect(item_cursor, MDBX_NEXT, key, 12));

  CHECK(cursor_set_range_expect(item_cursor, 520, 520, 0));
  for (uint64_t key = 520; key <= 700; ++key) {
    CHECK(mdbx_cursor_del(item_cursor, MDBX_CURRENT));
    if (key < 700)
      CHECK(cursor_expect_current(item_cursor, key + 1, 0));
  }
  CHECK(cursor_set_range_expect(item_check, 520, 701, 0));
  for (uint64_t key = 520; key <= 700; ++key) {
    CHECK(cursor_put_pattern(item_cursor, key, 13, scratch, MDBX_NOOVERWRITE));
    CHECK(cursor_set_key_expect(item_cursor, key, 13, NULL));
  }
  CHECK(cursor_set_range_expect(item_cursor, 520, 520, 13));
  for (uint64_t key = 521; key <= 700; ++key)
    CHECK(cursor_move_item_expect(item_cursor, MDBX_NEXT, key, 13));

  CHECK(dup_get_both_expect(dup_cursor, 44, 200, MDBX_GET_BOTH));
  CHECK(mdbx_cursor_del(dup_cursor, MDBX_CURRENT));
  CHECK(expect_dup_pair_notfound(dup_check, 44, 200));
  CHECK(dup_get_both_expect(dup_cursor, 44, 201, MDBX_GET_BOTH));
  for (uint64_t value = 300; value <= 420; ++value) {
    CHECK(dup_get_both_expect(dup_cursor, 44, value, MDBX_GET_BOTH));
    CHECK(mdbx_cursor_del(dup_cursor, MDBX_CURRENT));
  }
  CHECK(dup_set_key_expect(dup_cursor, 44, 0));
  CHECK(mdbx_cursor_count(dup_cursor, &duplicates));
  REQUIRE(duplicates == 646, "unexpected duplicate count after write-cursor duplicate deletes");

  for (uint64_t value = 300; value <= 420; ++value) {
    CHECK(cursor_put_dup_uint64(dup_cursor, 44, value));
    CHECK(dup_get_both_expect(dup_cursor, 44, value, MDBX_GET_BOTH));
  }
  CHECK(cursor_put_dup_uint64(dup_cursor, 44, 200));
  CHECK(dup_set_key_expect(dup_cursor, 44, 0));
  CHECK(mdbx_cursor_count(dup_cursor, &duplicates));
  REQUIRE(duplicates == 768, "unexpected duplicate count after write-cursor duplicate reinserts");

  CHECK(dup_set_key_expect(dup_cursor, 42, 0));
  CHECK(mdbx_cursor_del(dup_cursor, MDBX_ALLDUPS));
  CHECK(expect_dup_key_notfound(dup_check, 42));
  for (uint64_t value = 0; value < 64; ++value)
    CHECK(cursor_put_dup_uint64(dup_cursor, 42, value));
  CHECK(dup_set_key_expect(dup_cursor, 42, 0));
  CHECK(mdbx_cursor_count(dup_cursor, &duplicates));
  REQUIRE(duplicates == 64, "unexpected duplicate count after write-cursor alldups reinsertion");
  CHECK(dup_set_key_expect(dup_cursor, 44, 0));

  CHECK(mdbx_txn_abort(txn));
  txn = NULL;
  mdbx_cursor_close(dup_check);
  dup_check = NULL;
  mdbx_cursor_close(dup_cursor);
  dup_cursor = NULL;
  mdbx_cursor_close(item_check);
  item_check = NULL;
  mdbx_cursor_close(item_cursor);
  item_cursor = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_cursor_open(txn, items, &item_cursor));
  CHECK(mdbx_cursor_open(txn, dups, &dup_cursor));
  CHECK(cursor_set_key_expect(item_cursor, 31, 0, NULL));
  CHECK(cursor_set_key_expect(item_cursor, 120, 0, NULL));
  CHECK(cursor_set_key_expect(item_cursor, 260, 0, NULL));
  CHECK(cursor_set_key_expect(item_cursor, 520, 0, NULL));
  CHECK(cursor_set_key_expect(item_cursor, 700, 0, NULL));
  CHECK(dup_set_key_expect(dup_cursor, 42, 0));
  CHECK(mdbx_cursor_count(dup_cursor, &duplicates));
  REQUIRE(duplicates == 767, "aborted write-cursor churn changed key42 duplicate count");
  CHECK(dup_get_both_expect(dup_cursor, 42, 11, MDBX_GET_BOTH));
  CHECK(expect_dup_pair_notfound(dup_cursor, 42, 10));
  CHECK(expect_dup_key_notfound(dup_cursor, 43));
  CHECK(dup_get_both_expect(dup_cursor, 44, 200, MDBX_GET_BOTH));
  CHECK(mdbx_cursor_count(dup_cursor, &duplicates));
  REQUIRE(duplicates == 768, "aborted write-cursor churn changed key44 duplicate count");
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;
  mdbx_cursor_close(dup_cursor);
  dup_cursor = NULL;
  mdbx_cursor_close(item_cursor);
  item_cursor = NULL;

  return MDBX_SUCCESS;

bailout:
  if (dup_check)
    mdbx_cursor_close(dup_check);
  if (dup_cursor)
    mdbx_cursor_close(dup_cursor);
  if (item_check)
    mdbx_cursor_close(item_check);
  if (item_cursor)
    mdbx_cursor_close(item_cursor);
  if (txn)
    mdbx_txn_abort(txn);
  return rc;

bailout_rc:
  rc = fail_rc("write cursor invalidation churn operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int exercise_permuted_write_cursor_churn(MDBX_env *env, MDBX_dbi items, MDBX_dbi dups, void *scratch) {
  static const unsigned order[] = {17, 3,  29, 11, 23, 7,  31, 13, 19, 5,  27, 1,  21, 9,  25, 15,
                                   30, 12, 24, 6,  18, 0,  28, 10, 22, 4,  16, 8,  26, 2,  20, 14};
  static const uint64_t overflow_update_keys[] = {310, 341, 403, 434, 465};
  int rc;
  MDBX_txn *txn = NULL;
  MDBX_cursor *clean_holder = NULL, *dirty_holder = NULL, *item_writer = NULL, *item_check = NULL;
  MDBX_cursor *dup_holder = NULL, *dup_writer = NULL, *dup_check = NULL;
  MDBX_val clean_data = val(NULL, 0), dirty_data = val(NULL, 0);
  MDBX_val held_key = val(NULL, 0), held_data = val(NULL, 0);
  const void *clean_ptr = NULL, *dirty_ptr = NULL, *dup_ptr = NULL;
  size_t clean_len = 0, dirty_len = 0, dup_len = 0, duplicates = 0;

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_cursor_open(txn, items, &clean_holder));
  CHECK(mdbx_cursor_open(txn, items, &dirty_holder));
  CHECK(mdbx_cursor_open(txn, items, &item_writer));
  CHECK(mdbx_cursor_open(txn, items, &item_check));
  CHECK(mdbx_cursor_open(txn, dups, &dup_holder));
  CHECK(mdbx_cursor_open(txn, dups, &dup_writer));
  CHECK(mdbx_cursor_open(txn, dups, &dup_check));

  CHECK(cursor_set_key_expect(clean_holder, 31, 0, &clean_data));
  clean_ptr = clean_data.iov_base;
  clean_len = clean_data.iov_len;

  CHECK(cursor_set_key_expect(dirty_holder, 279, 0, NULL));
  CHECK(cursor_put_pattern(dirty_holder, 279, 21, scratch, MDBX_CURRENT));
  CHECK(cursor_set_key_expect(dirty_holder, 279, 21, &dirty_data));
  CHECK(expect_is_dirty(txn, dirty_data.iov_base, MDBX_RESULT_TRUE, "permuted dirty cursor value"));
  dirty_ptr = dirty_data.iov_base;
  dirty_len = dirty_data.iov_len;

  CHECK(dup_get_both_expect(dup_holder, 42, 123, MDBX_GET_BOTH));
  CHECK(mdbx_cursor_get(dup_holder, &held_key, &held_data, MDBX_GET_CURRENT));
  CHECK(dup_expect_pair(&held_key, &held_data, 42, 123));
  dup_ptr = held_data.iov_base;
  dup_len = held_data.iov_len;

  for (size_t i = 0; i < ARRAY_LENGTH(order); ++i) {
    const uint64_t key_value = 360 + order[i];
    MDBX_val key = val((void *)&key_value, sizeof(key_value));
    MDBX_val data = val(NULL, 0);
    CHECK(mdbx_cursor_get(item_writer, &key, &data, MDBX_SET_KEY));
    CHECK(mdbx_cursor_del(item_writer, MDBX_CURRENT));
  }
  CHECK(cursor_set_range_expect(item_check, 360, 392, 0));

  for (size_t i = ARRAY_LENGTH(order); i > 0; --i) {
    const uint64_t key_value = 360 + order[i - 1];
    CHECK(cursor_put_pattern(item_writer, key_value, 22, scratch, MDBX_NOOVERWRITE));
    CHECK(cursor_set_key_expect(item_check, key_value, 22, NULL));
  }

  for (size_t i = 0; i < ARRAY_LENGTH(overflow_update_keys); ++i) {
    const uint64_t key_value = overflow_update_keys[i];
    CHECK(cursor_set_key_expect(item_writer, key_value, 0, NULL));
    CHECK(cursor_put_pattern(item_writer, key_value, 23, scratch, MDBX_CURRENT));
    CHECK(cursor_set_key_expect(item_check, key_value, 23, NULL));
  }

  for (size_t i = 0; i < ARRAY_LENGTH(order); ++i) {
    const uint64_t value = 500 + order[i];
    CHECK(dup_get_both_expect(dup_writer, 44, value, MDBX_GET_BOTH));
    CHECK(mdbx_cursor_del(dup_writer, MDBX_CURRENT));
  }
  CHECK(dup_set_key_expect(dup_check, 44, 0));
  CHECK(mdbx_cursor_count(dup_check, &duplicates));
  REQUIRE(duplicates == 736, "unexpected duplicate count after permuted duplicate deletes");

  for (size_t i = ARRAY_LENGTH(order); i > 0; --i) {
    const uint64_t value = 500 + order[i - 1];
    CHECK(cursor_put_dup_uint64(dup_writer, 44, value));
    CHECK(dup_get_both_expect(dup_check, 44, value, MDBX_GET_BOTH));
  }
  CHECK(dup_set_key_expect(dup_check, 44, 0));
  CHECK(mdbx_cursor_count(dup_check, &duplicates));
  REQUIRE(duplicates == 768, "unexpected duplicate count after permuted duplicate reinserts");

  for (uint64_t value = 0; value < 96; ++value)
    CHECK(cursor_put_dup_uint64(dup_writer, 43, value));
  CHECK(dup_set_key_expect(dup_writer, 43, 0));
  CHECK(mdbx_cursor_count(dup_writer, &duplicates));
  REQUIRE(duplicates == 96, "unexpected duplicate count for temporary key43");
  CHECK(mdbx_cursor_del(dup_writer, MDBX_ALLDUPS));
  CHECK(expect_dup_key_notfound(dup_check, 43));

  REQUIRE(check_pattern(clean_ptr, clean_len, value_seed(31, 0)),
          "permuted write churn changed held clean cursor value");
  REQUIRE(check_pattern(dirty_ptr, dirty_len, value_seed(279, 21)),
          "permuted write churn changed held dirty cursor value");
  {
    uint64_t held_value = 0;
    MDBX_val held = val((void *)dup_ptr, dup_len);
    CHECK(dup_value_from_val(&held, &held_value));
    REQUIRE(held_value == 123, "permuted write churn changed held dupsort cursor value");
  }

  CHECK(mdbx_txn_abort(txn));
  txn = NULL;
  mdbx_cursor_close(dup_check);
  dup_check = NULL;
  mdbx_cursor_close(dup_writer);
  dup_writer = NULL;
  mdbx_cursor_close(dup_holder);
  dup_holder = NULL;
  mdbx_cursor_close(item_check);
  item_check = NULL;
  mdbx_cursor_close(item_writer);
  item_writer = NULL;
  mdbx_cursor_close(dirty_holder);
  dirty_holder = NULL;
  mdbx_cursor_close(clean_holder);
  clean_holder = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_cursor_open(txn, items, &item_check));
  CHECK(mdbx_cursor_open(txn, dups, &dup_check));
  CHECK(cursor_set_key_expect(item_check, 31, 0, NULL));
  CHECK(cursor_set_key_expect(item_check, 279, 0, NULL));
  CHECK(cursor_set_key_expect(item_check, 360, 0, NULL));
  CHECK(cursor_set_key_expect(item_check, 391, 0, NULL));
  CHECK(cursor_set_key_expect(item_check, 403, 0, NULL));
  CHECK(dup_get_both_expect(dup_check, 42, 123, MDBX_GET_BOTH));
  CHECK(dup_set_key_expect(dup_check, 44, 0));
  CHECK(mdbx_cursor_count(dup_check, &duplicates));
  REQUIRE(duplicates == 768, "aborted permuted churn changed key44 duplicate count");
  CHECK(expect_dup_key_notfound(dup_check, 43));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (dup_check)
    mdbx_cursor_close(dup_check);
  if (dup_writer)
    mdbx_cursor_close(dup_writer);
  if (dup_holder)
    mdbx_cursor_close(dup_holder);
  if (item_check)
    mdbx_cursor_close(item_check);
  if (item_writer)
    mdbx_cursor_close(item_writer);
  if (dirty_holder)
    mdbx_cursor_close(dirty_holder);
  if (clean_holder)
    mdbx_cursor_close(clean_holder);
  if (txn)
    mdbx_txn_abort(txn);
  return rc;

bailout_rc:
  rc = fail_rc("permuted write cursor churn operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int exercise_write_txn_cursor_value_lifetime(MDBX_env *env, MDBX_dbi items, MDBX_dbi dups, void *scratch) {
  int rc;
  MDBX_txn *txn = NULL;
  MDBX_cursor *item_holder = NULL, *item_scan = NULL, *dup_holder = NULL, *dup_scan = NULL;

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_cursor_open(txn, items, &item_holder));
  CHECK(mdbx_cursor_open(txn, items, &item_scan));
  CHECK(mdbx_cursor_open(txn, dups, &dup_holder));
  CHECK(mdbx_cursor_open(txn, dups, &dup_scan));

  {
    MDBX_val clean_data = val(NULL, 0);
    CHECK(cursor_set_key_expect(item_holder, 31, 0, &clean_data));
    const void *clean_ptr = clean_data.iov_base;
    const size_t clean_len = clean_data.iov_len;

    MDBX_val key = val(NULL, 0), data = val(NULL, 0);
    for (size_t i = 0; i < 160; ++i) {
      rc = mdbx_cursor_get(item_scan, &key, &data, i ? MDBX_NEXT : MDBX_FIRST);
      if (rc == MDBX_NOTFOUND)
        break;
      if (rc != MDBX_SUCCESS)
        goto bailout_rc;
    }
    CHECK(dup_get_both_expect(dup_scan, 44, 700, MDBX_GET_BOTH_RANGE));
    CHECK(dup_move_expect(dup_scan, MDBX_PREV_DUP, 44, 699));
    REQUIRE(check_pattern(clean_ptr, clean_len, value_seed(31, 0)),
            "write transaction clean cursor value changed after independent cursor movement");
  }

  CHECK(put_pattern(txn, items, 120, 4, scratch));
  {
    MDBX_val dirty_data = val(NULL, 0);
    CHECK(cursor_set_key_expect(item_holder, 120, 4, &dirty_data));
    CHECK(expect_is_dirty(txn, dirty_data.iov_base, MDBX_RESULT_TRUE, "write transaction dirty cursor value"));
    const void *dirty_ptr = dirty_data.iov_base;
    const size_t dirty_len = dirty_data.iov_len;

    uint64_t seek = 64;
    MDBX_val key = val(&seek, sizeof(seek)), data = val(NULL, 0);
    CHECK(mdbx_cursor_get(item_scan, &key, &data, MDBX_SET_RANGE));
    for (size_t i = 0; i < 160; ++i) {
      rc = mdbx_cursor_get(item_scan, &key, &data, MDBX_NEXT);
      if (rc == MDBX_NOTFOUND)
        break;
      if (rc != MDBX_SUCCESS)
        goto bailout_rc;
    }
    CHECK(dup_set_key_expect(dup_scan, 44, 0));
    CHECK(dup_move_expect(dup_scan, MDBX_LAST_DUP, 44, 767));
    CHECK(dup_move_expect(dup_scan, MDBX_PREV_DUP, 44, 766));
    REQUIRE(check_pattern(dirty_ptr, dirty_len, value_seed(120, 4)),
            "write transaction dirty cursor value changed after independent cursor movement");
  }

  {
    CHECK(dup_get_both_expect(dup_holder, 44, 123, MDBX_GET_BOTH));
    MDBX_val held_key = val(NULL, 0), held_data = val(NULL, 0);
    CHECK(mdbx_cursor_get(dup_holder, &held_key, &held_data, MDBX_GET_CURRENT));
    CHECK(dup_expect_pair(&held_key, &held_data, 44, 123));
    const void *held_ptr = held_data.iov_base;
    const size_t held_len = held_data.iov_len;

    CHECK(dup_set_key_expect(dup_scan, 42, 0));
    for (size_t i = 0; i < 128; ++i) {
      rc = mdbx_cursor_get(dup_scan, &held_key, &held_data, MDBX_NEXT_DUP);
      if (rc == MDBX_NOTFOUND)
        break;
      if (rc != MDBX_SUCCESS)
        goto bailout_rc;
    }
    CHECK(dup_set_key_expect(dup_scan, 44, 0));
    CHECK(dup_move_expect(dup_scan, MDBX_LAST_DUP, 44, 767));
    {
      uint64_t held_value = 0;
      MDBX_val held = val((void *)held_ptr, held_len);
      CHECK(dup_value_from_val(&held, &held_value));
      REQUIRE(held_value == 123, "write transaction dupsort cursor value was not retained");
    }
  }

  mdbx_cursor_close(dup_scan);
  dup_scan = NULL;
  mdbx_cursor_close(dup_holder);
  dup_holder = NULL;
  mdbx_cursor_close(item_scan);
  item_scan = NULL;
  mdbx_cursor_close(item_holder);
  item_holder = NULL;
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  return MDBX_SUCCESS;

bailout:
  if (dup_scan)
    mdbx_cursor_close(dup_scan);
  if (dup_holder)
    mdbx_cursor_close(dup_holder);
  if (item_scan)
    mdbx_cursor_close(item_scan);
  if (item_holder)
    mdbx_cursor_close(item_holder);
  if (txn)
    mdbx_txn_abort(txn);
  return rc;

bailout_rc:
  rc = fail_rc("write transaction cursor lifetime operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int exercise_nested_spill_unspill(MDBX_env *env, MDBX_dbi dbi, void *scratch) {
  int rc;
  MDBX_txn *txn = NULL;
  MDBX_txn *nested = NULL;
  MDBX_envinfo before, after;
  uint64_t saved_parent_spill = 0;
  int restore_parent_spill = 0;

  CHECK(mdbx_env_info_ex(env, NULL, &before, sizeof(before)));
  CHECK(mdbx_env_get_option(env, MDBX_opt_spill_parent4child_denominator, &saved_parent_spill));
  restore_parent_spill = 1;
  CHECK(mdbx_env_set_option(env, MDBX_opt_spill_parent4child_denominator, 1));

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  for (uint64_t key = 0; key < 760; ++key)
    CHECK(put_pattern(txn, dbi, key, 7, scratch));

  CHECK(mdbx_txn_begin(env, txn, 0, &nested));
  CHECK(put_pattern(nested, dbi, 10, 8, scratch));
  CHECK(put_pattern(nested, dbi, 700, 8, scratch));
  CHECK(mdbx_txn_abort(nested));
  nested = NULL;
  CHECK(expect_pattern(txn, dbi, 10, 7));
  CHECK(expect_pattern(txn, dbi, 700, 7));

  for (uint64_t key = 0; key < 380; ++key)
    CHECK(put_pattern(txn, dbi, key, 9, scratch));

  CHECK(mdbx_txn_begin(env, txn, 0, &nested));
  CHECK(put_pattern(nested, dbi, 300, 10, scratch));
  CHECK(put_pattern(nested, dbi, 301, 10, scratch));
  CHECK(mdbx_txn_commit(nested));
  nested = NULL;
  CHECK(expect_pattern(txn, dbi, 10, 9));
  CHECK(expect_pattern(txn, dbi, 300, 10));
  CHECK(expect_pattern(txn, dbi, 301, 10));
  CHECK(expect_pattern(txn, dbi, 700, 7));

  CHECK(mdbx_txn_abort(txn));
  txn = NULL;
  CHECK(mdbx_env_set_option(env, MDBX_opt_spill_parent4child_denominator, saved_parent_spill));
  restore_parent_spill = 0;

  CHECK(mdbx_env_info_ex(env, NULL, &after, sizeof(after)));
  REQUIRE(after.mi_pgop_stat.spill > before.mi_pgop_stat.spill, "nested spill stress did not spill pages");
  REQUIRE(after.mi_pgop_stat.unspill > before.mi_pgop_stat.unspill, "nested spill stress did not unspill pages");

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(expect_pattern(txn, dbi, 10, 0));
  CHECK(expect_pattern(txn, dbi, 300, 0));
  CHECK(expect_pattern(txn, dbi, 301, 0));
  CHECK(expect_pattern(txn, dbi, 700, 0));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  return MDBX_SUCCESS;

bailout:
  if (nested)
    mdbx_txn_abort(nested);
  if (txn)
    mdbx_txn_abort(txn);
  if (restore_parent_spill) {
    int restore_rc = mdbx_env_set_option(env, MDBX_opt_spill_parent4child_denominator, saved_parent_spill);
    if (rc == MDBX_SUCCESS && restore_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_set_option restore", restore_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("nested spill/unspill operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int exercise_permuted_nested_spill_churn(MDBX_env *env, const char *name, void *scratch) {
  enum { churn_records = 768, churn_inserts = 32 };
  const uint64_t base = UINT64_C(40000);
  const uint64_t insert_base = UINT64_C(41000);
  const uint64_t abort_insert_base = UINT64_C(41100);
  unsigned generation[churn_records];
  bool present[churn_records];
  MDBX_txn *txn = NULL;
  MDBX_txn *nested = NULL;
  MDBX_dbi churn = 0;
  MDBX_envinfo before, after;
  uint64_t saved_parent_spill = 0;
  int restore_parent_spill = 0;
  void *old_buffer = NULL;
  size_t expected_count = 0;
  size_t count = 0;
  int rc;

  old_buffer = malloc(12000 + 6 * 1024);
  if (!old_buffer)
    return fail_msg("malloc failed", __FILE__, __LINE__);

  for (size_t i = 0; i < churn_records; ++i) {
    generation[i] = 0;
    present[i] = true;
  }

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, name, MDBX_CREATE | MDBX_INTEGERKEY, &churn));
  for (uint64_t i = 0; i < churn_records; ++i) {
    const uint64_t key = base + i;
    if ((i & 15) == 0)
      CHECK(reserve_pattern(txn, churn, key, 0));
    else
      CHECK(put_pattern(txn, churn, key, 0, scratch));
  }
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  CHECK(mdbx_env_info_ex(env, NULL, &before, sizeof(before)));
  CHECK(mdbx_env_get_option(env, MDBX_opt_spill_parent4child_denominator, &saved_parent_spill));
  restore_parent_spill = 1;
  CHECK(mdbx_env_set_option(env, MDBX_opt_spill_parent4child_denominator, 1));

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, name, MDBX_DB_ACCEDE, &churn));
  for (size_t i = 0; i < churn_records; ++i) {
    const size_t slot = (i * 37u + 11u) % churn_records;
    CHECK(put_pattern(txn, churn, base + slot, 1, scratch));
    generation[slot] = 1;
  }

  CHECK(mdbx_txn_begin(env, txn, 0, &nested));
  for (size_t i = 0; i < 96; ++i) {
    const size_t slot = (i * 53u + 5u) % churn_records;
    const uint64_t key = base + slot;
    if ((slot % 17) == 0) {
      MDBX_val k = val((void *)&key, sizeof(key));
      CHECK(mdbx_del(nested, churn, &k, NULL));
    } else {
      CHECK(put_pattern(nested, churn, key, 7, scratch));
    }
  }
  for (uint64_t i = 0; i < 8; ++i)
    CHECK(put_pattern(nested, churn, abort_insert_base + i, 7, scratch));
  CHECK(mdbx_txn_abort(nested));
  nested = NULL;

  for (size_t i = 0; i < 96; ++i) {
    const size_t slot = (i * 53u + 5u) % churn_records;
    CHECK(expect_pattern(txn, churn, base + slot, 1));
  }
  for (uint64_t i = 0; i < 8; ++i)
    CHECK(expect_notfound(txn, churn, abort_insert_base + i));

  CHECK(mdbx_txn_begin(env, txn, 0, &nested));
  for (size_t i = 0; i < 192; ++i) {
    const size_t slot = (i * 97u + 17u) % churn_records;
    const uint64_t key = base + slot;
    MDBX_val k = val((void *)&key, sizeof(key));
    if ((slot % 13) == 0) {
      CHECK(mdbx_del(nested, churn, &k, NULL));
      present[slot] = false;
    } else {
      MDBX_val new_data = val(scratch, value_size(key));
      MDBX_val old_data = val(old_buffer, 12000 + 6 * 1024);
      fill_pattern(scratch, new_data.iov_len, value_seed(key, 2));
      CHECK(mdbx_replace(nested, churn, &k, &new_data, &old_data, 0));
      REQUIRE(old_data.iov_len == value_size(key), "nested replace returned wrong old value size");
      REQUIRE(check_pattern(old_data.iov_base, old_data.iov_len, value_seed(key, 1)),
              "nested replace returned wrong old value bytes");
      generation[slot] = 2;
    }
  }
  for (uint64_t i = 0; i < churn_inserts / 2; ++i)
    CHECK(put_pattern(nested, churn, insert_base + i, 2, scratch));
  CHECK(mdbx_txn_checkpoint(nested, MDBX_TXN_NOWEAKING, NULL));

  for (size_t i = 0; i < 160; ++i) {
    const size_t slot = (i * 193u + 23u) % churn_records;
    const uint64_t key = base + slot;
    CHECK(put_pattern(nested, churn, key, 3, scratch));
    generation[slot] = 3;
    present[slot] = true;
  }
  for (uint64_t i = churn_inserts / 2; i < churn_inserts; ++i)
    CHECK(put_pattern(nested, churn, insert_base + i, 3, scratch));
  CHECK(mdbx_txn_commit(nested));
  nested = NULL;

  for (size_t slot = 0; slot < churn_records; ++slot) {
    if (present[slot])
      CHECK(expect_pattern(txn, churn, base + slot, generation[slot]));
    else
      CHECK(expect_notfound(txn, churn, base + slot));
  }
  for (uint64_t i = 0; i < churn_inserts / 2; ++i)
    CHECK(expect_pattern(txn, churn, insert_base + i, 2));
  for (uint64_t i = churn_inserts / 2; i < churn_inserts; ++i)
    CHECK(expect_pattern(txn, churn, insert_base + i, 3));
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  CHECK(mdbx_env_set_option(env, MDBX_opt_spill_parent4child_denominator, saved_parent_spill));
  restore_parent_spill = 0;

  CHECK(mdbx_env_info_ex(env, NULL, &after, sizeof(after)));
  REQUIRE(after.mi_pgop_stat.spill > before.mi_pgop_stat.spill, "permuted nested spill churn did not spill pages");
  REQUIRE(after.mi_pgop_stat.unspill > before.mi_pgop_stat.unspill,
          "permuted nested spill churn did not unspill pages");

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_dbi_open(txn, name, MDBX_DB_ACCEDE, &churn));
  for (size_t slot = 0; slot < churn_records; ++slot) {
    if (present[slot]) {
      CHECK(expect_pattern(txn, churn, base + slot, generation[slot]));
      ++expected_count;
    } else {
      CHECK(expect_notfound(txn, churn, base + slot));
    }
  }
  for (uint64_t i = 0; i < churn_inserts / 2; ++i) {
    CHECK(expect_pattern(txn, churn, insert_base + i, 2));
    ++expected_count;
  }
  for (uint64_t i = churn_inserts / 2; i < churn_inserts; ++i) {
    CHECK(expect_pattern(txn, churn, insert_base + i, 3));
    ++expected_count;
  }
  CHECK(count_records(txn, churn, &count));
  REQUIRE(count == expected_count, "unexpected permuted nested spill churn count");
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, name, MDBX_DB_ACCEDE, &churn));
  CHECK(mdbx_drop(txn, churn, true));
  churn = 0;
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  free(old_buffer);
  return MDBX_SUCCESS;

bailout:
  if (nested)
    mdbx_txn_abort(nested);
  if (txn)
    mdbx_txn_abort(txn);
  if (restore_parent_spill) {
    int restore_rc = mdbx_env_set_option(env, MDBX_opt_spill_parent4child_denominator, saved_parent_spill);
    if (rc == MDBX_SUCCESS && restore_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_set_option restore", restore_rc, __FILE__, __LINE__);
  }
  if (churn)
    mdbx_dbi_close(env, churn);
  free(old_buffer);
  return rc;

bailout_rc:
  rc = fail_rc("permuted nested spill churn operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int exercise_cursor_value_lifetime(MDBX_txn *txn, MDBX_dbi dbi) {
  int rc;
  MDBX_cursor *holder = NULL, *scanner = NULL;
  uint64_t held_key_value = 31;
  MDBX_val held_key = val(&held_key_value, sizeof(held_key_value));
  MDBX_val held_data = val(NULL, 0);

  CHECK(mdbx_cursor_open(txn, dbi, &holder));
  CHECK(mdbx_cursor_get(holder, &held_key, &held_data, MDBX_SET_KEY));
  REQUIRE(held_data.iov_len == value_size(held_key_value), "unexpected held cursor value size");
  REQUIRE(check_pattern(held_data.iov_base, held_data.iov_len, value_seed(held_key_value, 0)),
          "held cursor value mismatch before read-only cursor movement");

  CHECK(mdbx_cursor_open(txn, dbi, &scanner));
  MDBX_val scan_key = val(NULL, 0), scan_data = val(NULL, 0);
  for (size_t i = 0; i < 128; ++i) {
    rc = mdbx_cursor_get(scanner, &scan_key, &scan_data, i ? MDBX_NEXT : MDBX_FIRST);
    if (rc == MDBX_NOTFOUND)
      break;
    if (rc != MDBX_SUCCESS)
      goto bailout_rc;
  }

  CHECK(expect_pattern(txn, dbi, 5000, 0));
  REQUIRE(check_pattern(held_data.iov_base, held_data.iov_len, value_seed(held_key_value, 0)),
          "held cursor value changed after independent read-only operations");

  rc = MDBX_SUCCESS;

bailout:
  if (scanner)
    mdbx_cursor_close(scanner);
  if (holder)
    mdbx_cursor_close(holder);
  return rc;

bailout_rc:
  rc = fail_rc("cursor lifetime operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int exercise_write_cursor_churn(MDBX_env *env, MDBX_dbi dbi, void *scratch, size_t records) {
  int rc;
  MDBX_txn *txn = NULL;
  MDBX_cursor *cursor = NULL;
  const uint64_t updated_key = 31;

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_cursor_open(txn, dbi, &cursor));
  CHECK(cursor_set_key_expect(cursor, updated_key, 0, NULL));
  MDBX_val key = val((void *)&updated_key, sizeof(updated_key));
  MDBX_val data = val(scratch, value_size(updated_key));
  fill_pattern(scratch, data.iov_len, value_seed(updated_key, 3));
  CHECK(mdbx_cursor_put(cursor, &key, &data, MDBX_CURRENT));
  CHECK(cursor_expect_current(cursor, updated_key, 3));

  CHECK(mdbx_cursor_del(cursor, MDBX_CURRENT));
  CHECK(cursor_expect_current(cursor, updated_key + 1, 0));
  CHECK(put_pattern(txn, dbi, updated_key, 3, scratch));

  for (uint64_t i = 6000; i < 6200; ++i) {
    MDBX_val delete_key = val(&i, sizeof(i));
    MDBX_val delete_data = val(NULL, 0);
    CHECK(mdbx_cursor_get(cursor, &delete_key, &delete_data, MDBX_SET_KEY));
    CHECK(mdbx_cursor_del(cursor, MDBX_CURRENT));
  }
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;
  mdbx_cursor_close(cursor);
  cursor = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(expect_pattern(txn, dbi, updated_key, 3));
  CHECK(expect_notfound(txn, dbi, 6000));
  CHECK(expect_notfound(txn, dbi, 6199));
  size_t count = 0;
  CHECK(count_records(txn, dbi, &count));
  REQUIRE(count == records - 200, "unexpected record count after write-cursor delete churn");
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  for (uint64_t i = 6000; i < 6200; ++i)
    CHECK(put_pattern(txn, dbi, i, 3, scratch));
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(expect_pattern(txn, dbi, updated_key, 3));
  CHECK(expect_pattern(txn, dbi, 6000, 3));
  CHECK(expect_pattern(txn, dbi, 6199, 3));
  CHECK(count_records(txn, dbi, &count));
  REQUIRE(count == records, "unexpected record count after write-cursor reinsertion");
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (cursor)
    mdbx_cursor_close(cursor);
  if (txn)
    mdbx_txn_abort(txn);
  return rc;

bailout_rc:
  rc = fail_rc("write cursor churn operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static size_t overflow_value_size(uint64_t key, unsigned generation) {
  return 14000 + (size_t)((key + generation) % 8) * 512;
}

static uint64_t overflow_value_seed(uint64_t key, unsigned generation) {
  return value_seed(key + UINT64_C(0xC000), generation + 17);
}

static int put_overflow_pattern(MDBX_txn *txn, MDBX_dbi dbi, uint64_t key, unsigned generation, void *scratch) {
  MDBX_val k = val(&key, sizeof(key));
  MDBX_val data = val(scratch, overflow_value_size(key, generation));
  fill_pattern(scratch, data.iov_len, overflow_value_seed(key, generation));
  return mdbx_put(txn, dbi, &k, &data, 0);
}

static int reserve_overflow_pattern(MDBX_txn *txn, MDBX_dbi dbi, uint64_t key, unsigned generation) {
  MDBX_val k = val(&key, sizeof(key));
  MDBX_val data = val(NULL, overflow_value_size(key, generation));
  int rc = mdbx_put(txn, dbi, &k, &data, MDBX_RESERVE);
  if (rc == MDBX_SUCCESS)
    fill_pattern(data.iov_base, data.iov_len, overflow_value_seed(key, generation));
  return rc;
}

static int expect_overflow_pattern(MDBX_txn *txn, MDBX_dbi dbi, uint64_t key, unsigned generation) {
  MDBX_val k = val(&key, sizeof(key));
  MDBX_val data = val(NULL, 0);
  int rc = mdbx_get(txn, dbi, &k, &data);
  if (rc != MDBX_SUCCESS)
    return fail_rc("mdbx_get overflow", rc, __FILE__, __LINE__);
  if (data.iov_len != overflow_value_size(key, generation))
    return fail_msg("unexpected overflow value size", __FILE__, __LINE__);
  if (!check_pattern(data.iov_base, data.iov_len, overflow_value_seed(key, generation)))
    return fail_msg("unexpected overflow value bytes", __FILE__, __LINE__);
  return MDBX_SUCCESS;
}

static int check_overflow_stat(MDBX_txn *txn, MDBX_dbi dbi, size_t expected_entries) {
  int rc;
  MDBX_stat stat;

  CHECK(mdbx_dbi_stat(txn, dbi, &stat, sizeof(stat)));
  REQUIRE(stat.ms_entries == expected_entries, "unexpected overflow table entry count");
  REQUIRE(stat.ms_overflow_pages > 0, "overflow table did not report overflow pages");
  return MDBX_SUCCESS;

bailout:
  return rc;

bailout_rc:
  rc = fail_rc("overflow stat operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int verify_overflow_after_defrag(MDBX_txn *txn, MDBX_dbi dbi) {
  int rc;
  size_t count = 0;

  CHECK(check_overflow_stat(txn, dbi, 81));
  CHECK(count_records(txn, dbi, &count));
  REQUIRE(count == 81, "unexpected overflow record count");
  CHECK(expect_overflow_pattern(txn, dbi, 1, 0));
  CHECK(expect_overflow_pattern(txn, dbi, 2, 2));
  CHECK(expect_overflow_pattern(txn, dbi, 31, 1));
  CHECK(expect_notfound(txn, dbi, 32));
  CHECK(expect_notfound(txn, dbi, 47));
  CHECK(expect_overflow_pattern(txn, dbi, 64, 2));
  CHECK(expect_overflow_pattern(txn, dbi, 95, 2));
  CHECK(expect_overflow_pattern(txn, dbi, 96, 2));
  return MDBX_SUCCESS;

bailout:
  return rc;

bailout_rc:
  rc = fail_rc("overflow verification operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int exercise_overflow_defrag(MDBX_env *env, const char *overflow_name, void *scratch) {
  enum { overflow_scratch_bytes = 12000 + 6 * 1024 };
  MDBX_txn *txn = NULL;
  MDBX_txn *nested = NULL;
  MDBX_dbi overflow = 0;
  void *old_buffer = NULL;
  int rc;

  old_buffer = malloc(overflow_scratch_bytes);
  if (!old_buffer)
    return fail_msg("malloc failed", __FILE__, __LINE__);

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, overflow_name, MDBX_CREATE | MDBX_INTEGERKEY, &overflow));
  for (uint64_t key = 0; key < 64; ++key) {
    if (key % 9 == 0)
      CHECK(reserve_overflow_pattern(txn, overflow, key, 0));
    else
      CHECK(put_overflow_pattern(txn, overflow, key, 0, scratch));
  }
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(check_overflow_stat(txn, overflow, 64));
  CHECK(expect_overflow_pattern(txn, overflow, 0, 0));
  CHECK(expect_overflow_pattern(txn, overflow, 31, 0));
  {
    uint64_t held_key_value = 9;
    MDBX_val held_key = val(&held_key_value, sizeof(held_key_value));
    MDBX_val held_data = val(NULL, 0);
    CHECK(mdbx_get(txn, overflow, &held_key, &held_data));
    REQUIRE(held_data.iov_len == overflow_value_size(held_key_value, 0), "unexpected held overflow size");
    const void *held_ptr = held_data.iov_base;
    const size_t held_len = held_data.iov_len;
    for (uint64_t key = 10; key < 48; ++key)
      CHECK(expect_overflow_pattern(txn, overflow, key, 0));
    REQUIRE(check_pattern(held_ptr, held_len, overflow_value_seed(held_key_value, 0)),
            "held overflow value was not retained across overflow gets");
  }
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  {
    const uint64_t key_value = 31;
    MDBX_val key = val((void *)&key_value, sizeof(key_value));
    MDBX_val new_data = val(scratch, overflow_value_size(key_value, 1));
    MDBX_val old_data = val(old_buffer, overflow_scratch_bytes);
    fill_pattern(scratch, new_data.iov_len, overflow_value_seed(key_value, 1));
    CHECK(mdbx_replace(txn, overflow, &key, &new_data, &old_data, 0));
    REQUIRE(old_data.iov_len == overflow_value_size(key_value, 0), "replace returned wrong old overflow size");
    REQUIRE(check_pattern(old_data.iov_base, old_data.iov_len, overflow_value_seed(key_value, 0)),
            "replace returned wrong old overflow bytes");
  }
  for (uint64_t key = 32; key < 48; ++key) {
    MDBX_val k = val(&key, sizeof(key));
    CHECK(mdbx_del(txn, overflow, &k, NULL));
  }
  for (uint64_t key = 64; key < 96; ++key)
    CHECK(put_overflow_pattern(txn, overflow, key, 2, scratch));

  CHECK(mdbx_txn_begin(env, txn, 0, &nested));
  CHECK(put_overflow_pattern(nested, overflow, 1, 2, scratch));
  CHECK(put_overflow_pattern(nested, overflow, 96, 2, scratch));
  CHECK(mdbx_txn_abort(nested));
  nested = NULL;
  CHECK(expect_overflow_pattern(txn, overflow, 1, 0));
  CHECK(expect_notfound(txn, overflow, 96));

  CHECK(mdbx_txn_begin(env, txn, 0, &nested));
  CHECK(put_overflow_pattern(nested, overflow, 2, 2, scratch));
  CHECK(put_overflow_pattern(nested, overflow, 96, 2, scratch));
  CHECK(mdbx_txn_commit(nested));
  nested = NULL;
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(verify_overflow_after_defrag(txn, overflow));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  {
    MDBX_defrag_result_t result;
    memset(&result, 0, sizeof(result));
    rc = mdbx_env_defrag(env, 0, 0, 0, 0, -1, 8, NULL, NULL, &result);
    if (rc != MDBX_SUCCESS && rc != MDBX_RESULT_TRUE)
      goto bailout_rc;
    REQUIRE((result.stopping_reasons & MDBX_defrag_error) == 0, "defrag reported an error stop reason");
    rc = MDBX_SUCCESS;
  }

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(verify_overflow_after_defrag(txn, overflow));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (nested)
    mdbx_txn_abort(nested);
  if (txn)
    mdbx_txn_abort(txn);
  if (overflow)
    mdbx_dbi_close(env, overflow);
  free(old_buffer);
  return rc;

bailout_rc:
  rc = fail_rc("overflow defrag operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static size_t churn_overflow_value_size(uint64_t key, unsigned generation) {
  static const size_t sizes[] = {8193, 12289, 18433, 24577, 32769, 49153, 65537, 73729};
  const size_t slot = (size_t)((key * UINT64_C(3) + generation * UINT64_C(5)) % ARRAY_LENGTH(sizes));
  return sizes[slot] + (size_t)((key ^ (uint64_t)generation * UINT64_C(131)) & 127);
}

static size_t churn_overflow_max_value_size(void) {
  return 73729 + 127;
}

static uint64_t churn_overflow_seed(uint64_t key, unsigned generation) {
  return value_seed(key + UINT64_C(0xBAD000), generation + 41);
}

static int put_churn_overflow_pattern(MDBX_txn *txn, MDBX_dbi dbi, uint64_t key, unsigned generation, void *scratch) {
  MDBX_val k = val(&key, sizeof(key));
  MDBX_val data = val(scratch, churn_overflow_value_size(key, generation));
  fill_pattern(scratch, data.iov_len, churn_overflow_seed(key, generation));
  return mdbx_put(txn, dbi, &k, &data, 0);
}

static int reserve_churn_overflow_pattern(MDBX_txn *txn, MDBX_dbi dbi, uint64_t key, unsigned generation) {
  MDBX_val k = val(&key, sizeof(key));
  MDBX_val data = val(NULL, churn_overflow_value_size(key, generation));
  int rc = mdbx_put(txn, dbi, &k, &data, MDBX_RESERVE);
  if (rc == MDBX_SUCCESS)
    fill_pattern(data.iov_base, data.iov_len, churn_overflow_seed(key, generation));
  return rc;
}

static int expect_churn_overflow_pattern(MDBX_txn *txn, MDBX_dbi dbi, uint64_t key, unsigned generation) {
  MDBX_val k = val(&key, sizeof(key));
  MDBX_val data = val(NULL, 0);
  int rc = mdbx_get(txn, dbi, &k, &data);
  if (rc != MDBX_SUCCESS)
    return fail_rc("mdbx_get randomized overflow", rc, __FILE__, __LINE__);
  if (data.iov_len != churn_overflow_value_size(key, generation))
    return fail_msg("unexpected randomized overflow value size", __FILE__, __LINE__);
  if (!check_pattern(data.iov_base, data.iov_len, churn_overflow_seed(key, generation)))
    return fail_msg("unexpected randomized overflow value bytes", __FILE__, __LINE__);
  return MDBX_SUCCESS;
}

static int exercise_randomized_overflow_churn(MDBX_env *env, const char *overflow_name) {
  enum { churn_records = 32, churn_inserts = 16 };
  static const unsigned order[churn_records] = {17, 3,  29, 11, 23, 7,  31, 13, 19, 5,  27, 1,  21, 9,  25, 15,
                                               30, 12, 24, 6,  18, 0,  28, 10, 22, 4,  16, 8,  26, 2,  20, 14};
  const uint64_t base = UINT64_C(20000);
  const uint64_t insert_base = UINT64_C(20128);
  const uint64_t aborted_key = UINT64_C(20777);
  const uint64_t committed_key = UINT64_C(20778);
  unsigned generation[churn_records];
  bool present[churn_records];
  size_t base_count = 0;
  size_t count = 0;
  int rc;
  MDBX_txn *txn = NULL;
  MDBX_txn *pinned = NULL;
  MDBX_txn *nested = NULL;
  MDBX_dbi overflow = 0;
  void *scratch = NULL;
  void *old_buffer = NULL;

  const size_t scratch_bytes = churn_overflow_max_value_size();
  scratch = malloc(scratch_bytes);
  old_buffer = malloc(scratch_bytes);
  if (!scratch || !old_buffer) {
    rc = fail_msg("malloc failed", __FILE__, __LINE__);
    goto bailout;
  }

  for (size_t i = 0; i < churn_records; ++i) {
    generation[i] = 0;
    present[i] = true;
  }

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, overflow_name, MDBX_DB_ACCEDE, &overflow));
  CHECK(count_records(txn, overflow, &base_count));
  for (uint64_t i = 0; i < churn_records; ++i) {
    const uint64_t key = base + i;
    if ((i & 3) == 0)
      CHECK(reserve_churn_overflow_pattern(txn, overflow, key, 0));
    else
      CHECK(put_churn_overflow_pattern(txn, overflow, key, 0, scratch));
  }
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &pinned));
  {
    const uint64_t held_key_value = base + 7;
    MDBX_val held_key = val((void *)&held_key_value, sizeof(held_key_value));
    MDBX_val held_data = val(NULL, 0);
    CHECK(mdbx_get(pinned, overflow, &held_key, &held_data));
    REQUIRE(held_data.iov_len == churn_overflow_value_size(held_key_value, 0),
            "unexpected randomized held overflow size");
    const void *held_ptr = held_data.iov_base;
    const size_t held_len = held_data.iov_len;

    CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
    for (size_t i = 0; i < ARRAY_LENGTH(order); ++i) {
      const unsigned slot = order[i];
      const uint64_t key = base + slot;
      if (slot % 7 == 0) {
        MDBX_val k = val((void *)&key, sizeof(key));
        CHECK(mdbx_del(txn, overflow, &k, NULL));
        present[slot] = false;
      } else if (slot % 5 == 0) {
        MDBX_val k = val((void *)&key, sizeof(key));
        MDBX_val new_data = val(scratch, churn_overflow_value_size(key, 2));
        MDBX_val old_data = val(old_buffer, scratch_bytes);
        fill_pattern(scratch, new_data.iov_len, churn_overflow_seed(key, 2));
        CHECK(mdbx_replace(txn, overflow, &k, &new_data, &old_data, 0));
        REQUIRE(old_data.iov_len == churn_overflow_value_size(key, 0),
                "replace returned wrong randomized old overflow size");
        REQUIRE(check_pattern(old_data.iov_base, old_data.iov_len, churn_overflow_seed(key, 0)),
                "replace returned wrong randomized old overflow bytes");
        generation[slot] = 2;
      } else if (slot % 3 == 0) {
        CHECK(put_churn_overflow_pattern(txn, overflow, key, 1, scratch));
        generation[slot] = 1;
      }
    }

    for (uint64_t i = 0; i < churn_inserts; ++i) {
      const uint64_t key = insert_base + i;
      if ((i & 1) == 0)
        CHECK(reserve_churn_overflow_pattern(txn, overflow, key, 3));
      else
        CHECK(put_churn_overflow_pattern(txn, overflow, key, 3, scratch));
    }

    CHECK(mdbx_txn_begin(env, txn, 0, &nested));
    CHECK(put_churn_overflow_pattern(nested, overflow, base + 1, 7, scratch));
    CHECK(put_churn_overflow_pattern(nested, overflow, aborted_key, 7, scratch));
    CHECK(mdbx_txn_abort(nested));
    nested = NULL;
    CHECK(expect_churn_overflow_pattern(txn, overflow, base + 1, 0));
    CHECK(expect_notfound(txn, overflow, aborted_key));

    CHECK(mdbx_txn_begin(env, txn, 0, &nested));
    CHECK(put_churn_overflow_pattern(nested, overflow, base + 2, 6, scratch));
    CHECK(put_churn_overflow_pattern(nested, overflow, committed_key, 6, scratch));
    CHECK(mdbx_txn_commit(nested));
    nested = NULL;
    generation[2] = 6;
    CHECK(mdbx_txn_commit(txn));
    txn = NULL;

    REQUIRE(check_pattern(held_ptr, held_len, churn_overflow_seed(held_key_value, 0)),
            "randomized overflow value was not retained across writer churn");
    CHECK(expect_churn_overflow_pattern(pinned, overflow, held_key_value, 0));
  }
  CHECK(mdbx_txn_abort(pinned));
  pinned = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  for (size_t slot = 0; slot < churn_records; ++slot) {
    const uint64_t key = base + slot;
    if (present[slot])
      CHECK(expect_churn_overflow_pattern(txn, overflow, key, generation[slot]));
    else
      CHECK(expect_notfound(txn, overflow, key));
  }
  for (uint64_t i = 0; i < churn_inserts; ++i)
    CHECK(expect_churn_overflow_pattern(txn, overflow, insert_base + i, 3));
  CHECK(expect_notfound(txn, overflow, aborted_key));
  CHECK(expect_churn_overflow_pattern(txn, overflow, committed_key, 6));
  CHECK(count_records(txn, overflow, &count));
  REQUIRE(count == base_count + churn_records - 5 + churn_inserts + 1,
          "unexpected randomized overflow record count");
  CHECK(check_overflow_stat(txn, overflow, count));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (nested)
    mdbx_txn_abort(nested);
  if (txn)
    mdbx_txn_abort(txn);
  if (pinned)
    mdbx_txn_abort(pinned);
  if (overflow)
    mdbx_dbi_close(env, overflow);
  free(old_buffer);
  free(scratch);
  return rc;

bailout_rc:
  rc = fail_rc("randomized overflow churn operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static uint64_t gc_stress_key(uint64_t base, size_t index) {
  return base + (uint64_t)index * 31;
}

static int exercise_gc_reuse_retention(MDBX_env *env, const char *name, void *scratch) {
  const size_t reuse_records = 128;
  const size_t reuse_growth_margin = 32;
  const uint64_t pinned_base = UINT64_C(62000);
  const uint64_t reuse_base = UINT64_C(93000);
  int rc;
  MDBX_txn *txn = NULL;
  MDBX_txn *pinned = NULL;
  MDBX_dbi gc = 0;
  MDBX_gc_info_t before_retire, retained, reclaimable, before_reuse, after_reuse;

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, name, MDBX_CREATE | MDBX_INTEGERKEY, &gc));
  for (size_t i = 0; i < reuse_records; ++i)
    CHECK(put_pattern(txn, gc, gc_stress_key(pinned_base, i), 0, scratch));
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &pinned));
  CHECK(expect_pattern(pinned, gc, gc_stress_key(pinned_base, 0), 0));
  CHECK(expect_pattern(pinned, gc, gc_stress_key(pinned_base, reuse_records - 1), 0));

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(get_gc_info(txn, &before_retire));
  for (size_t i = 0; i < reuse_records; ++i) {
    uint64_t key_value = gc_stress_key(pinned_base, i);
    MDBX_val key = val(&key_value, sizeof(key_value));
    CHECK(mdbx_del(txn, gc, &key, NULL));
  }
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  CHECK(expect_pattern(pinned, gc, gc_stress_key(pinned_base, reuse_records - 1), 0));

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(expect_notfound(txn, gc, gc_stress_key(pinned_base, 0)));
  CHECK(get_gc_info(txn, &retained));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;
  REQUIRE(retained.pages_gc > before_retire.pages_gc, "delete wave did not enter GC");
  REQUIRE(retained.gc_reclaimable.pages < retained.pages_gc, "pinned reader did not retain deleted pages");

  CHECK(mdbx_txn_abort(pinned));
  pinned = NULL;
#if !defined(_WIN32) && !defined(_WIN64)
  {
    int dead = 0;
    CHECK(mdbx_reader_check(env, &dead));
  }
#endif

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(get_gc_info(txn, &reclaimable));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;
  REQUIRE(reclaimable.gc_reclaimable.pages > retained.gc_reclaimable.pages,
          "released reader did not make retired pages reclaimable");

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(get_gc_info(txn, &before_reuse));
  for (size_t i = 0; i < reuse_records; ++i)
    CHECK(put_pattern(txn, gc, gc_stress_key(reuse_base, i), 1, scratch));
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(get_gc_info(txn, &after_reuse));
  CHECK(expect_pattern(txn, gc, gc_stress_key(reuse_base, 0), 1));
  CHECK(expect_pattern(txn, gc, gc_stress_key(reuse_base, reuse_records - 1), 1));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;
  REQUIRE(after_reuse.pages_allocated <= before_reuse.pages_allocated + reuse_growth_margin,
          "reuse wave allocated too many new pages instead of reusing GC pages");

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  for (size_t i = 0; i < reuse_records; ++i) {
    uint64_t key_value = gc_stress_key(reuse_base, i);
    MDBX_val key = val(&key_value, sizeof(key_value));
    CHECK(mdbx_del(txn, gc, &key, NULL));
  }
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  mdbx_dbi_close(env, gc);
  return MDBX_SUCCESS;

bailout:
  if (pinned)
    mdbx_txn_abort(pinned);
  if (txn)
    mdbx_txn_abort(txn);
  if (gc)
    mdbx_dbi_close(env, gc);
  return rc;

bailout_rc:
  rc = fail_rc("GC reuse/retention operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int exercise_resize_gc(MDBX_env *env, MDBX_dbi dbi, void *scratch, size_t records) {
  int rc;
  MDBX_txn *txn = NULL;
  MDBX_envinfo before, after;
  size_t count = 0;

  CHECK(mdbx_env_info_ex(env, NULL, &before, sizeof(before)));
  CHECK(grow_active_geometry(env, 256 * 1024, "geometry current size did not grow"));
  CHECK(mdbx_env_info_ex(env, NULL, &after, sizeof(after)));
  REQUIRE(after.mi_geo.current > before.mi_geo.current, "geometry current size did not grow");
  REQUIRE(after.mi_geo.upper == before.mi_geo.upper, "geometry upper bound changed unexpectedly");

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  for (uint64_t i = 200; i < 400; ++i) {
    MDBX_val key = val(&i, sizeof(i));
    CHECK(mdbx_del(txn, dbi, &key, NULL));
  }
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(count_records(txn, dbi, &count));
  REQUIRE(count == records - 200, "unexpected record count after deletion wave");
  CHECK(expect_notfound(txn, dbi, 250));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  for (uint64_t i = 6000; i < 6200; ++i)
    CHECK(put_pattern(txn, dbi, i, 0, scratch));
  for (uint64_t i = 400; i < 600; ++i)
    CHECK(put_pattern(txn, dbi, i, 2, scratch));
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(count_records(txn, dbi, &count));
  REQUIRE(count == records, "unexpected record count after GC/reuse wave");
  CHECK(expect_notfound(txn, dbi, 250));
  CHECK(expect_pattern(txn, dbi, 400, 2));
  CHECK(expect_pattern(txn, dbi, 599, 2));
  CHECK(expect_pattern(txn, dbi, 6000, 0));
  CHECK(expect_pattern(txn, dbi, 6199, 0));
  CHECK(exercise_cursor_value_lifetime(txn, dbi));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  CHECK(shrink_active_geometry(env, 64 * 1024, "geometry current size did not shrink"));

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(count_records(txn, dbi, &count));
  REQUIRE(count == records, "unexpected record count after geometry shrink");
  CHECK(expect_notfound(txn, dbi, 250));
  CHECK(expect_pattern(txn, dbi, 400, 2));
  CHECK(expect_pattern(txn, dbi, 6000, 0));
  CHECK(expect_pattern(txn, dbi, 6199, 0));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  return rc;

bailout_rc:
  rc = fail_rc("resize/gc operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int verify_copy_path(const char *path, const char *items_name, size_t records) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  size_t count = 0;
  int rc;

  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_maxdbs(env, 4));
  CHECK(mdbx_env_open(env, path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_RDONLY, 0664));
  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(count_records(txn, items, &count));
  REQUIRE(count == records, "unexpected record count in copied environment");
  CHECK(expect_pattern(txn, items, 17, 1));
  CHECK(expect_notfound(txn, items, 23));
  CHECK(expect_notfound(txn, items, 250));
  CHECK(expect_pattern(txn, items, 31, 3));
  CHECK(expect_pattern(txn, items, 400, 2));
  CHECK(expect_pattern(txn, items, 599, 2));
  CHECK(expect_pattern(txn, items, 6000, 3));
  CHECK(expect_pattern(txn, items, 6199, 3));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("copy verification operation", rc, __FILE__, __LINE__);
  goto bailout;
}

#if !defined(_WIN32) && !defined(_WIN64)
static int close_copy_pipe_fd(int *fd) {
  if (*fd < 0)
    return MDBX_SUCCESS;
  for (;;) {
    if (close(*fd) == 0) {
      *fd = -1;
      return MDBX_SUCCESS;
    }
    if (errno != EINTR)
      break;
  }
  const int err = errno;
  *fd = -1;
  return fail_errno("close(copy pipe)", err, __FILE__, __LINE__);
}

typedef struct copy_pipe_drain_ctx {
  int fd;
  int err;
  size_t bytes;
} copy_pipe_drain_ctx_t;

static void *drain_copy_pipe_thread(void *arg) {
  copy_pipe_drain_ctx_t *const ctx = (copy_pipe_drain_ctx_t *)arg;
  unsigned char buffer[8192];

  for (;;) {
    const ssize_t got = read(ctx->fd, buffer, sizeof(buffer));
    if (got > 0) {
      ctx->bytes += (size_t)got;
      continue;
    }
    if (got == 0)
      break;
    if (errno == EINTR)
      continue;
    ctx->err = errno;
    return NULL;
  }

  ctx->err = ctx->bytes > 0 ? MDBX_SUCCESS : MDBX_PROBLEM;
  return NULL;
}

static int join_copy_pipe_thread(pthread_t thread, copy_pipe_drain_ctx_t *ctx) {
  const int join_err = pthread_join(thread, NULL);
  if (join_err)
    return fail_errno("pthread_join(copy pipe)", join_err, __FILE__, __LINE__);
  if (ctx->err == MDBX_SUCCESS)
    return MDBX_SUCCESS;
  if (ctx->err > 0)
    return fail_errno("read(copy pipe)", ctx->err, __FILE__, __LINE__);
  return fail_msg("copy pipe produced no bytes", __FILE__, __LINE__);
}

static int exercise_env_copy2fd_pipe(MDBX_env *env) {
  int pipefd[2] = {-1, -1};
  pthread_t drain_thread;
  bool drain_started = false;
  copy_pipe_drain_ctx_t drain_ctx = {pipefd[0], MDBX_SUCCESS, 0};
  int rc = MDBX_SUCCESS;

  if (pipe(pipefd) != 0)
    return fail_errno("pipe(copy)", errno, __FILE__, __LINE__);
  drain_ctx.fd = pipefd[0];
  rc = pthread_create(&drain_thread, NULL, drain_copy_pipe_thread, &drain_ctx);
  if (rc) {
    rc = fail_errno("pthread_create(copy pipe)", rc, __FILE__, __LINE__);
    goto bailout;
  }
  drain_started = true;
  CHECK(mdbx_env_copy2fd(env, pipefd[1], MDBX_CP_DEFAULTS));
  CHECK(close_copy_pipe_fd(&pipefd[1]));
  CHECK(join_copy_pipe_thread(drain_thread, &drain_ctx));
  drain_started = false;
  CHECK(close_copy_pipe_fd(&pipefd[0]));

  return MDBX_SUCCESS;

bailout:
  (void)close_copy_pipe_fd(&pipefd[1]);
  if (drain_started) {
    int wait_rc = join_copy_pipe_thread(drain_thread, &drain_ctx);
    if (rc == MDBX_SUCCESS)
      rc = wait_rc;
  }
  (void)close_copy_pipe_fd(&pipefd[0]);
  return rc;

bailout_rc:
  rc = fail_rc("copy pipe operation", rc, __FILE__, __LINE__);
  goto bailout;
}
#endif /* !Windows */

static int exercise_env_copy(MDBX_env *env, const char *name, const char *items_name, size_t records) {
  static const struct {
    const char *suffix;
    MDBX_copy_flags_t flags;
  } copies[] = {
      {"copy", MDBX_CP_DEFAULTS},
      {"compact-copy", MDBX_CP_COMPACT | MDBX_CP_FORCE_DYNAMIC_SIZE},
  };
  char path[160];
  int rc = MDBX_SUCCESS;

  for (size_t i = 0; i < ARRAY_LENGTH(copies); ++i) {
    snprintf(path, sizeof(path), "./migration-smoke-%lu-%s-%s", smoke_run_id(), copies[i].suffix, name);
    rc = mdbx_env_delete(path, MDBX_ENV_JUST_DELETE);
    if (rc != MDBX_SUCCESS && rc != MDBX_RESULT_TRUE)
      return fail_rc("mdbx_env_delete copy target", rc, __FILE__, __LINE__);

    rc = mdbx_env_copy(env, path, copies[i].flags);
    if (rc != MDBX_SUCCESS)
      return fail_rc("mdbx_env_copy", rc, __FILE__, __LINE__);

    rc = verify_copy_path(path, items_name, records);
    if (rc != MDBX_SUCCESS)
      return rc;

    rc = mdbx_env_delete(path, MDBX_ENV_JUST_DELETE);
    if (rc != MDBX_SUCCESS && rc != MDBX_RESULT_TRUE)
      return fail_rc("mdbx_env_delete copy cleanup", rc, __FILE__, __LINE__);
  }

#if !defined(_WIN32) && !defined(_WIN64)
  rc = exercise_env_copy2fd_pipe(env);
  if (rc != MDBX_SUCCESS)
    return rc;
#endif /* !Windows */

  return MDBX_SUCCESS;
}

static int select_explicit_profile(int tinycache) {
  int rc;

  if (tinycache) {
    rc = set_env_var("MDBX_FORCE_NO_DATA_MMAP", "1");
    if (rc != MDBX_SUCCESS)
      return rc;
    return set_env_var("MDBX_EXPLICIT_PAGE_CACHE_LIMIT", "64K");
  }

  rc = unset_env_var("MDBX_FORCE_NO_DATA_MMAP");
  if (rc != MDBX_SUCCESS)
    return rc;
  rc = unset_env_var("MDBX_EXPLICIT_PAGE_CACHE_LIMIT");
  return rc;
}

static int delete_explicit_profile_path(const char *path) {
  const int rc = mdbx_env_delete(path, MDBX_ENV_JUST_DELETE);
  return (rc == MDBX_SUCCESS || rc == MDBX_RESULT_TRUE) ? MDBX_SUCCESS
                                                        : fail_rc("mdbx_env_delete", rc, __FILE__, __LINE__);
}

static int create_explicit_profile_seed(const char *path, const char *items_name, uint64_t first_key, size_t records,
                                        void *scratch) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  int rc;

  CHECK(delete_explicit_profile_path(path));
  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_maxdbs(env, 2));
  CHECK(mdbx_env_set_geometry(env, 0, 0, 16 * 1024 * 1024, 64 * 1024, 64 * 1024, -1));
  CHECK(mdbx_env_set_option(env, MDBX_opt_txn_dp_limit, 128));
  CHECK(mdbx_env_set_option(env, MDBX_opt_txn_dp_initial, 128));
  CHECK(mdbx_env_open(env, path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM, 0664));
  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_CREATE | MDBX_INTEGERKEY, &items));
  for (size_t i = 0; i < records; ++i)
    CHECK(put_pattern(txn, items, first_key + (uint64_t)i, 0, scratch));
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("explicit-profile seed operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int verify_explicit_profile_records(const char *path, const char *items_name, uint64_t first_key, size_t records,
                                           uint64_t updated_key, unsigned updated_generation) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  size_t count = 0;
  int rc;

  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_maxdbs(env, 2));
  CHECK(mdbx_env_open(env, path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_RDONLY | MDBX_LIFORECLAIM, 0664));
  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(count_records(txn, items, &count));
  REQUIRE(count == records, "explicit-profile record count changed");
  for (size_t i = 0; i < records; ++i) {
    const uint64_t key = first_key + (uint64_t)i;
    const unsigned generation = (key == updated_key) ? updated_generation : 0;
    CHECK(expect_pattern(txn, items, key, generation));
  }
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("explicit-profile verification operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int update_explicit_profile_record(const char *path, const char *items_name, uint64_t key, unsigned generation,
                                          void *scratch) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  int rc;

  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_maxdbs(env, 2));
  CHECK(mdbx_env_set_option(env, MDBX_opt_txn_dp_limit, 128));
  CHECK(mdbx_env_set_option(env, MDBX_opt_txn_dp_initial, 128));
  CHECK(mdbx_env_open(env, path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM, 0664));
  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(put_pattern(txn, items, key, generation, scratch));
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("explicit-profile update operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int exercise_explicit_profile_compatibility(void) {
  const char *items_name = "items";
  const size_t records = 192;
  const uint64_t default_first = UINT64_C(100000);
  const uint64_t tinycache_first = UINT64_C(200000);
  const uint64_t no_update = UINT64_MAX;
  char default_path[160];
  char tinycache_path[160];
  struct saved_env_var saved_force = {"MDBX_FORCE_NO_DATA_MMAP", NULL, 0};
  struct saved_env_var saved_limit = {"MDBX_EXPLICIT_PAGE_CACHE_LIMIT", NULL, 0};
  int saved_force_ready = 0;
  int saved_limit_ready = 0;
  void *scratch = NULL;
  int rc;

  snprintf(default_path, sizeof(default_path), "./migration-smoke-%lu-explicit-default", smoke_run_id());
  snprintf(tinycache_path, sizeof(tinycache_path), "./migration-smoke-%lu-explicit-tinycache", smoke_run_id());

  scratch = malloc(12000 + 6 * 1024);
  if (!scratch)
    return fail_msg("malloc failed", __FILE__, __LINE__);

  CHECK(save_env_var(&saved_force, "MDBX_FORCE_NO_DATA_MMAP"));
  saved_force_ready = 1;
  CHECK(save_env_var(&saved_limit, "MDBX_EXPLICIT_PAGE_CACHE_LIMIT"));
  saved_limit_ready = 1;

  CHECK(select_explicit_profile(0));
  CHECK(create_explicit_profile_seed(default_path, items_name, default_first, records, scratch));
  CHECK(select_explicit_profile(1));
  CHECK(verify_explicit_profile_records(default_path, items_name, default_first, records, no_update, 0));
  CHECK(update_explicit_profile_record(default_path, items_name, default_first + 31, 1, scratch));
  CHECK(verify_explicit_profile_records(default_path, items_name, default_first, records, default_first + 31, 1));
  CHECK(select_explicit_profile(0));
  CHECK(verify_explicit_profile_records(default_path, items_name, default_first, records, default_first + 31, 1));
  CHECK(delete_explicit_profile_path(default_path));

  CHECK(select_explicit_profile(1));
  CHECK(create_explicit_profile_seed(tinycache_path, items_name, tinycache_first, records, scratch));
  CHECK(select_explicit_profile(0));
  CHECK(verify_explicit_profile_records(tinycache_path, items_name, tinycache_first, records, no_update, 0));
  CHECK(update_explicit_profile_record(tinycache_path, items_name, tinycache_first + 63, 2, scratch));
  CHECK(verify_explicit_profile_records(tinycache_path, items_name, tinycache_first, records, tinycache_first + 63, 2));
  CHECK(select_explicit_profile(1));
  CHECK(verify_explicit_profile_records(tinycache_path, items_name, tinycache_first, records, tinycache_first + 63, 2));
  CHECK(delete_explicit_profile_path(tinycache_path));

  rc = MDBX_SUCCESS;

bailout:
  (void)delete_explicit_profile_path(default_path);
  (void)delete_explicit_profile_path(tinycache_path);
  if (saved_limit_ready) {
    int restore_rc = restore_env_var(&saved_limit);
    if (rc == MDBX_SUCCESS && restore_rc != MDBX_SUCCESS)
      rc = restore_rc;
  }
  if (saved_force_ready) {
    int restore_rc = restore_env_var(&saved_force);
    if (rc == MDBX_SUCCESS && restore_rc != MDBX_SUCCESS)
      rc = restore_rc;
  }
  free(scratch);
  return rc;

bailout_rc:
  rc = fail_rc("explicit-profile compatibility operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int exercise_lckless_readonly_nommap(void) {
#if defined(_WIN32) || defined(_WIN64)
  return MDBX_SUCCESS;
#else
  const char *items_name = "items";
  const size_t records = 128;
  const uint64_t first_key = UINT64_C(300000);
  char path[160];
  char lck_path[sizeof(path) + sizeof(MDBX_LOCK_SUFFIX)];
  struct saved_env_var saved_force = {"MDBX_FORCE_NO_DATA_MMAP", NULL, 0};
  struct saved_env_var saved_limit = {"MDBX_EXPLICIT_PAGE_CACHE_LIMIT", NULL, 0};
  int saved_force_ready = 0;
  int saved_limit_ready = 0;
  void *scratch = NULL;
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  struct stat lck_stat;
  int lck_mode_saved = 0;
  int lck_mode_restricted = 0;
  size_t count = 0;
  int rc;

  snprintf(path, sizeof(path), "./migration-smoke-%lu-lckless", smoke_run_id());
  snprintf(lck_path, sizeof(lck_path), "%s" MDBX_LOCK_SUFFIX, path);

  scratch = malloc(12000 + 6 * 1024);
  if (!scratch)
    return fail_msg("malloc failed", __FILE__, __LINE__);

  CHECK(save_env_var(&saved_force, "MDBX_FORCE_NO_DATA_MMAP"));
  saved_force_ready = 1;
  CHECK(save_env_var(&saved_limit, "MDBX_EXPLICIT_PAGE_CACHE_LIMIT"));
  saved_limit_ready = 1;

  CHECK(select_explicit_profile(1));
  CHECK(create_explicit_profile_seed(path, items_name, first_key, records, scratch));
  if (stat(lck_path, &lck_stat) != 0) {
    rc = fail_errno("stat lck file", errno, __FILE__, __LINE__);
    goto bailout;
  }
  lck_mode_saved = 1;
  if (chmod(lck_path, 0) != 0) {
    rc = fail_errno("chmod lck file", errno, __FILE__, __LINE__);
    goto bailout;
  }
  lck_mode_restricted = 1;

  if (access(lck_path, R_OK | W_OK) == 0) {
    fprintf(stderr, "skip lckless-readonly: chmod did not make the lck file inaccessible\n");
    rc = MDBX_SUCCESS;
    goto bailout;
  }

  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_maxdbs(env, 2));
  CHECK(mdbx_env_open(env, path,
                      MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_RDONLY | MDBX_EXCLUSIVE | MDBX_LIFORECLAIM,
                      0664));
  rc = mdbx_env_sync_ex(env, true, false);
  REQUIRE(rc == MDBX_EACCESS, "lckless read-only sync did not reject the read-only environment");
  rc = MDBX_SUCCESS;
  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(count_records(txn, items, &count));
  REQUIRE(count == records, "lckless read-only record count changed");
  CHECK(expect_pattern(txn, items, first_key, 0));
  CHECK(expect_pattern(txn, items, first_key + 31, 0));
  CHECK(expect_pattern(txn, items, first_key + (uint64_t)records - 1, 0));
  CHECK(check_env_info_snapshot(env, txn));
  CHECK(check_txn_observer_apis(txn, 0, 1));

  struct reader_list_state state;
  memset(&state, 0, sizeof(state));
  rc = mdbx_reader_list(env, reader_list_count, &state);
  REQUIRE(rc == MDBX_RESULT_TRUE, "lckless reader list did not report the lock-free path");
  REQUIRE(state.entries == 0 && state.active == 0, "lckless reader list unexpectedly reported lock slots");
  rc = MDBX_SUCCESS;

  int dead = -1;
  CHECK(mdbx_reader_check(env, &dead));
  REQUIRE(dead == 0, "lckless reader check reported dead readers");
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  if ((lck_mode_restricted || lck_mode_saved) && chmod(lck_path, lck_stat.st_mode & 07777) != 0 &&
      rc == MDBX_SUCCESS)
    rc = fail_errno("restore lck file mode", errno, __FILE__, __LINE__);
  {
    int delete_rc = delete_explicit_profile_path(path);
    if (rc == MDBX_SUCCESS && delete_rc != MDBX_SUCCESS)
      rc = delete_rc;
  }
  if (saved_limit_ready) {
    int restore_rc = restore_env_var(&saved_limit);
    if (rc == MDBX_SUCCESS && restore_rc != MDBX_SUCCESS)
      rc = restore_rc;
  }
  if (saved_force_ready) {
    int restore_rc = restore_env_var(&saved_force);
    if (rc == MDBX_SUCCESS && restore_rc != MDBX_SUCCESS)
      rc = restore_rc;
  }
  free(scratch);
  return rc;

bailout_rc:
  rc = fail_rc("lckless read-only no-map operation", rc, __FILE__, __LINE__);
  goto bailout;
#endif
}

static int exercise_mode(const char *name, MDBX_env_flags_t flags, int writemap_optional) {
  const size_t records = 900;
  const char *items_name = "items";
  const char *dups_name = "dups";
  char path[128];
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_txn *read_txn = NULL;
  MDBX_dbi items = 0;
  MDBX_dbi dups = 0;
  void *scratch = NULL;
  int rc;

  snprintf(path, sizeof(path), "./migration-smoke-%lu-%s", smoke_run_id(), name);
  rc = mdbx_env_delete(path, MDBX_ENV_JUST_DELETE);
  if (rc != MDBX_SUCCESS && rc != MDBX_RESULT_TRUE)
    return fail_rc("mdbx_env_delete", rc, __FILE__, __LINE__);

  scratch = malloc(12000 + 6 * 1024);
  if (!scratch)
    return fail_msg("malloc failed", __FILE__, __LINE__);

  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_maxdbs(env, 4));
  CHECK(mdbx_env_set_geometry(env, 0, 0, 32 * 1024 * 1024, 64 * 1024, 64 * 1024, -1));
  CHECK(mdbx_env_set_option(env, MDBX_opt_txn_dp_limit, 128));
  CHECK(mdbx_env_set_option(env, MDBX_opt_txn_dp_initial, 128));
  CHECK(mdbx_env_set_option(env, MDBX_opt_spill_min_denominator, 2));
  CHECK(mdbx_env_set_option(env, MDBX_opt_spill_max_denominator, 2));

  rc = mdbx_env_open(env, path, flags, 0664);
  if (rc != MDBX_SUCCESS) {
    if (writemap_optional) {
      REQUIRE(rc == MDBX_INCOMPATIBLE, "explicit-I/O policy did not reject MDBX_WRITEMAP as incompatible");
      fprintf(stderr, "skip %s: MDBX_WRITEMAP unsupported by the explicit-I/O backend (%d, %s)\n", name, rc,
              mdbx_strerror(rc));
      rc = MDBX_SUCCESS;
      goto bailout;
    }
    goto bailout_rc;
  }

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_CREATE | MDBX_INTEGERKEY, &items));
  CHECK(mdbx_dbi_open(txn, dups_name, MDBX_CREATE | MDBX_INTEGERKEY | MDBX_DUPSORT | MDBX_DUPFIXED |
                                           MDBX_INTEGERDUP,
                      &dups));
  for (uint64_t i = 0; i < records; ++i) {
    if (i % 97 == 0)
      CHECK(reserve_pattern(txn, items, i, 0));
    else
      CHECK(put_pattern(txn, items, i, 0, scratch));
  }
  CHECK(populate_dupsort(txn, dups));
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;
  CHECK(check_env_info_snapshot(env, NULL));
  CHECK(exercise_recovery_open_reject_active(env, path, items, scratch));
  CHECK(exercise_active_open_option_rejections(env));
  CHECK(mdbx_env_warmup(env, NULL, MDBX_warmup_default, 0));
  CHECK(mdbx_env_warmup(env, NULL, MDBX_warmup_force | MDBX_warmup_oomsafe, 0));
  if ((flags & MDBX_WRITEMAP) == 0) {
    rc = mdbx_env_warmup(env, NULL, MDBX_warmup_lock, 0);
    REQUIRE(rc == MDBX_ENOSYS, "no-data-mmap lock warmup did not report MDBX_ENOSYS");
    rc = MDBX_SUCCESS;
  }
  CHECK(exercise_dupsort_subcursor(env, dups));
  if ((flags & MDBX_WRITEMAP) == 0)
    CHECK(exercise_write_txn_cursor_value_lifetime(env, items, dups, scratch));
  if ((flags & MDBX_WRITEMAP) == 0) {
    CHECK(exercise_nested_spill_unspill(env, items, scratch));
    CHECK(exercise_permuted_nested_spill_churn(env, "spillstress", scratch));
  }

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &read_txn));
  CHECK(check_env_info_snapshot(env, read_txn));
  CHECK(check_txn_observer_apis(read_txn, 0, 1));
  CHECK(check_reader_list_snapshot(env, 1, 0));
  CHECK(expect_pattern(read_txn, items, 0, 0));
  CHECK(expect_pattern(read_txn, items, 31, 0));
  CHECK(expect_pattern(read_txn, items, 868, 0));
  CHECK(exercise_cursorless_value_lifetime(read_txn, items));

  MDBX_val stable_key, stable_data;
  uint64_t stable_key_value = 17;
  stable_key = val(&stable_key_value, sizeof(stable_key_value));
  stable_data = val(NULL, 0);
  CHECK(mdbx_get(read_txn, items, &stable_key, &stable_data));
  REQUIRE(stable_data.iov_len == value_size(stable_key_value), "unexpected stable value size");
  REQUIRE(check_pattern(stable_data.iov_base, stable_data.iov_len, value_seed(stable_key_value, 0)),
          "stable value mismatch before concurrent write");

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(put_pattern(txn, items, stable_key_value, 1, scratch));
  {
    uint64_t deleted = 23;
    MDBX_val deleted_key = val(&deleted, sizeof(deleted));
    CHECK(mdbx_del(txn, items, &deleted_key, NULL));
  }
  CHECK(put_pattern(txn, items, 5000, 0, scratch));
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  REQUIRE(check_pattern(stable_data.iov_base, stable_data.iov_len, value_seed(stable_key_value, 0)),
          "read snapshot value was not stable across concurrent commit");
  CHECK(check_txn_observer_apis(read_txn, 1, 0));
  CHECK(check_reader_list_snapshot(env, 1, 1));
  {
    MDBX_txn *amended = read_txn;
    rc = mdbx_txn_amend(read_txn, &amended, MDBX_TXN_TRY, NULL);
    REQUIRE(rc == MDBX_RESULT_TRUE, "amend did not reject stale read snapshot");
    REQUIRE(amended == read_txn, "amend changed transaction handle after stale snapshot rejection");
  }
  CHECK(expect_pattern(read_txn, items, stable_key_value, 0));
  CHECK(expect_pattern(read_txn, items, 23, 0));
  CHECK(expect_notfound(read_txn, items, 5000));
  CHECK(mdbx_txn_abort(read_txn));
  read_txn = NULL;

  CHECK(exercise_cursor_invalidation_churn(env, items, dups));
  if ((flags & MDBX_WRITEMAP) == 0)
    CHECK(exercise_write_cursor_invalidation_churn(env, items, dups, scratch));
  if ((flags & MDBX_WRITEMAP) == 0)
    CHECK(exercise_permuted_write_cursor_churn(env, items, dups, scratch));

  if ((flags & MDBX_WRITEMAP) == 0) {
    MDBX_txn *nested = NULL;
    CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
    CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
    CHECK(put_pattern(txn, items, 100, 1, scratch));
    {
      uint64_t dirty_key_value = 100;
      MDBX_val dirty_key = val(&dirty_key_value, sizeof(dirty_key_value));
      MDBX_val dirty_data = val(NULL, 0);
      CHECK(mdbx_get(txn, items, &dirty_key, &dirty_data));
      CHECK(expect_is_dirty(txn, dirty_data.iov_base, MDBX_RESULT_TRUE, "write transaction dirty value"));
    }

    CHECK(mdbx_txn_begin(env, txn, 0, &nested));
    CHECK(put_pattern(nested, items, 101, 1, scratch));
    CHECK(put_pattern(nested, items, 9000, 0, scratch));
    CHECK(mdbx_txn_abort(nested));
    nested = NULL;
    CHECK(expect_pattern(txn, items, 101, 0));
    CHECK(expect_notfound(txn, items, 9000));

    CHECK(mdbx_txn_begin(env, txn, 0, &nested));
    CHECK(put_pattern(nested, items, 102, 1, scratch));
    CHECK(mdbx_txn_commit(nested));
    nested = NULL;
    CHECK(mdbx_txn_commit(txn));
    txn = NULL;
  }

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &read_txn));
  CHECK(check_txn_observer_apis(read_txn, 0, 1));
  CHECK(expect_pattern(read_txn, items, stable_key_value, 1));
  CHECK(expect_notfound(read_txn, items, 23));
  CHECK(expect_pattern(read_txn, items, 5000, 0));
  if ((flags & MDBX_WRITEMAP) == 0) {
    CHECK(expect_pattern(read_txn, items, 100, 1));
    CHECK(expect_pattern(read_txn, items, 101, 0));
    CHECK(expect_pattern(read_txn, items, 102, 1));
    CHECK(expect_notfound(read_txn, items, 9000));
  }
  size_t count = 0;
  CHECK(count_records(read_txn, items, &count));
  REQUIRE(count == records, "unexpected record count after updates");
  CHECK(mdbx_txn_abort(read_txn));
  read_txn = NULL;

  CHECK(exercise_cursor_copy_reset_renew(env, items, stable_key_value, 1, 5000, 0));
  if ((flags & MDBX_WRITEMAP) == 0)
    CHECK(exercise_gc_reuse_retention(env, "gcstress", scratch));
  CHECK(exercise_resize_gc(env, items, scratch, records));
  CHECK(exercise_write_cursor_churn(env, items, scratch, records));
  if ((flags & MDBX_WRITEMAP) == 0) {
    CHECK(exercise_overflow_defrag(env, "overflow", scratch));
    CHECK(exercise_randomized_overflow_churn(env, "overflow"));
  }
  CHECK(exercise_env_copy(env, name, items_name, records));
  CHECK(exercise_manual_sync(env, items, scratch));

  MDBX_envinfo info;
  CHECK(mdbx_env_info_ex(env, NULL, &info, sizeof(info)));
  CHECK(check_env_info_snapshot(env, NULL));
  REQUIRE((info.mi_mode & MDBX_WRITEMAP) == 0, "shared envmode unexpectedly advertised MDBX_WRITEMAP");
  if ((flags & MDBX_WRITEMAP) == 0)
    REQUIRE(info.mi_pgop_stat.spill > 0, "dirty-page spill path was not exercised");

  rc = MDBX_SUCCESS;

bailout:
  if (read_txn)
    mdbx_txn_abort(read_txn);
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (dups && env)
    mdbx_dbi_close(env, dups);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  free(scratch);
  if (rc == MDBX_SUCCESS) {
    int delete_rc = mdbx_env_delete(path, MDBX_ENV_JUST_DELETE);
    if (delete_rc != MDBX_SUCCESS && delete_rc != MDBX_RESULT_TRUE)
      rc = fail_rc("mdbx_env_delete cleanup", delete_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("mdbx call", rc, __FILE__, __LINE__);
  goto bailout;
}

static int exercise_accede_open_policy(void) {
  char path[128];
  MDBX_env *env = NULL;
  int rc;

  snprintf(path, sizeof(path), "./migration-smoke-%lu-accede-open", smoke_run_id());
  rc = mdbx_env_delete(path, MDBX_ENV_JUST_DELETE);
  if (rc != MDBX_SUCCESS && rc != MDBX_RESULT_TRUE)
    return fail_rc("mdbx_env_delete", rc, __FILE__, __LINE__);

  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_geometry(env, 0, 0, 4 * 1024 * 1024, 64 * 1024, 64 * 1024, -1));
  CHECK(mdbx_env_open(env, path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM | MDBX_ACCEDE, 0664));

  MDBX_envinfo info;
  CHECK(mdbx_env_info_ex(env, NULL, &info, sizeof(info)));
  REQUIRE((info.mi_mode & MDBX_WRITEMAP) == 0, "MDBX_ACCEDE open unexpectedly advertised MDBX_WRITEMAP");

  rc = MDBX_SUCCESS;

bailout:
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  if (rc == MDBX_SUCCESS) {
    int delete_rc = mdbx_env_delete(path, MDBX_ENV_JUST_DELETE);
    if (delete_rc != MDBX_SUCCESS && delete_rc != MDBX_RESULT_TRUE)
      rc = fail_rc("mdbx_env_delete cleanup", delete_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("MDBX_ACCEDE open policy operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int exercise_writemap_set_flags_policy(void) {
  char path[128];
  MDBX_env *env = NULL;
  int rc;

  snprintf(path, sizeof(path), "./migration-smoke-%lu-writemap-setflags", smoke_run_id());
  rc = mdbx_env_delete(path, MDBX_ENV_JUST_DELETE);
  if (rc != MDBX_SUCCESS && rc != MDBX_RESULT_TRUE)
    return fail_rc("mdbx_env_delete", rc, __FILE__, __LINE__);

  CHECK(mdbx_env_create(&env));
  rc = mdbx_env_set_flags(env, MDBX_WRITEMAP, true);
  REQUIRE(rc == MDBX_INCOMPATIBLE, "inactive MDBX_WRITEMAP set_flags was not rejected as incompatible");
  rc = MDBX_SUCCESS;

  unsigned flags = 0;
  CHECK(mdbx_env_get_flags(env, &flags));
  REQUIRE((flags & MDBX_WRITEMAP) == 0, "rejected inactive MDBX_WRITEMAP set_flags left the flag enabled");

  CHECK(mdbx_env_set_geometry(env, 0, 0, 4 * 1024 * 1024, 64 * 1024, 64 * 1024, -1));
  CHECK(mdbx_env_open(env, path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM, 0664));

  MDBX_envinfo info;
  CHECK(mdbx_env_info_ex(env, NULL, &info, sizeof(info)));
  REQUIRE((info.mi_mode & MDBX_WRITEMAP) == 0, "normal open after rejected set_flags advertised MDBX_WRITEMAP");

  rc = mdbx_env_set_flags(env, MDBX_WRITEMAP, true);
  REQUIRE(rc == MDBX_INCOMPATIBLE, "active MDBX_WRITEMAP set_flags was not rejected as incompatible");
  rc = MDBX_SUCCESS;

  CHECK(mdbx_env_get_flags(env, &flags));
  REQUIRE((flags & MDBX_WRITEMAP) == 0, "rejected active MDBX_WRITEMAP set_flags left the flag enabled");

  rc = MDBX_SUCCESS;

bailout:
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  if (rc == MDBX_SUCCESS) {
    int delete_rc = mdbx_env_delete(path, MDBX_ENV_JUST_DELETE);
    if (delete_rc != MDBX_SUCCESS && delete_rc != MDBX_RESULT_TRUE)
      rc = fail_rc("mdbx_env_delete cleanup", delete_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("MDBX_WRITEMAP set_flags policy operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int exercise_legacy_mapasync_policy(void) {
  char path[128];
  MDBX_env *env = NULL;
  int rc;

  snprintf(path, sizeof(path), "./migration-smoke-%lu-legacy-mapasync-open", smoke_run_id());
  rc = mdbx_env_delete(path, MDBX_ENV_JUST_DELETE);
  if (rc != MDBX_SUCCESS && rc != MDBX_RESULT_TRUE)
    return fail_rc("mdbx_env_delete", rc, __FILE__, __LINE__);

  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_geometry(env, 0, 0, 4 * 1024 * 1024, 64 * 1024, 64 * 1024, -1));
  CHECK(mdbx_env_open(env, path,
                      MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM |
                          (MDBX_env_flags_t)MDBX_TEST_LEGACY_MAPASYNC,
                      0664));
  MDBX_envinfo info;
  CHECK(mdbx_env_info_ex(env, NULL, &info, sizeof(info)));
  REQUIRE((info.mi_mode & MDBX_TEST_LEGACY_MAPASYNC) == 0, "legacy MAPASYNC open kept deprecated bit");
  REQUIRE((info.mi_mode & MDBX_SAFE_NOSYNC) != 0, "legacy MAPASYNC open did not enable SAFE_NOSYNC");
  REQUIRE((info.mi_mode & MDBX_NOMETASYNC) != 0, "legacy MAPASYNC open did not enable NOMETASYNC");

  rc = mdbx_env_close(env);
  env = NULL;
  if (rc != MDBX_SUCCESS)
    goto bailout_rc;
  rc = mdbx_env_delete(path, MDBX_ENV_JUST_DELETE);
  if (rc != MDBX_SUCCESS && rc != MDBX_RESULT_TRUE)
    goto bailout_rc;

  snprintf(path, sizeof(path), "./migration-smoke-%lu-legacy-mapasync-setflags", smoke_run_id());
  rc = mdbx_env_delete(path, MDBX_ENV_JUST_DELETE);
  if (rc != MDBX_SUCCESS && rc != MDBX_RESULT_TRUE)
    return fail_rc("mdbx_env_delete", rc, __FILE__, __LINE__);

  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_geometry(env, 0, 0, 4 * 1024 * 1024, 64 * 1024, 64 * 1024, -1));
  CHECK(mdbx_env_open(env, path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM, 0664));
  CHECK(mdbx_env_set_flags(env, (MDBX_env_flags_t)MDBX_TEST_LEGACY_MAPASYNC, true));
  unsigned flags;
  CHECK(mdbx_env_get_flags(env, &flags));
  REQUIRE((flags & MDBX_TEST_LEGACY_MAPASYNC) == 0, "legacy MAPASYNC set_flags kept deprecated bit");
  REQUIRE((flags & MDBX_SAFE_NOSYNC) != 0, "legacy MAPASYNC set_flags did not enable SAFE_NOSYNC");
  REQUIRE((flags & MDBX_NOMETASYNC) != 0, "legacy MAPASYNC set_flags did not enable NOMETASYNC");

  rc = MDBX_SUCCESS;

bailout:
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  if (rc == MDBX_SUCCESS) {
    int delete_rc = mdbx_env_delete(path, MDBX_ENV_JUST_DELETE);
    if (delete_rc != MDBX_SUCCESS && delete_rc != MDBX_RESULT_TRUE)
      rc = fail_rc("mdbx_env_delete cleanup", delete_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("legacy MAPASYNC policy operation", rc, __FILE__, __LINE__);
  goto bailout;
}

#if defined(MIGRATION_FAULT_INJECTION)

static int delete_fault_path(const char *path) {
  const int rc = mdbx_env_delete(path, MDBX_ENV_JUST_DELETE);
  return (rc == MDBX_SUCCESS || rc == MDBX_RESULT_TRUE) ? MDBX_SUCCESS
                                                        : fail_rc("mdbx_env_delete", rc, __FILE__, __LINE__);
}

static int create_fault_seed(const char *path, const char *items_name, void *scratch) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  int rc;

  CHECK(delete_fault_path(path));
  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_maxdbs(env, 2));
  CHECK(mdbx_env_set_geometry(env, 0, 0, 16 * 1024 * 1024, 64 * 1024, 64 * 1024, -1));
  CHECK(mdbx_env_set_option(env, MDBX_opt_txn_dp_limit, 128));
  CHECK(mdbx_env_set_option(env, MDBX_opt_txn_dp_initial, 128));
  CHECK(mdbx_env_set_option(env, MDBX_opt_spill_min_denominator, 2));
  CHECK(mdbx_env_set_option(env, MDBX_opt_spill_max_denominator, 2));
  CHECK(mdbx_env_open(env, path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM, 0664));
  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_CREATE | MDBX_INTEGERKEY, &items));
  for (uint64_t key = 0; key < 192; ++key)
    CHECK(put_pattern(txn, items, key, 0, scratch));
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("fault seed operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int verify_fault_seed(const char *path, const char *items_name) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  size_t count = 0;
  int rc;

  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_maxdbs(env, 2));
  CHECK(mdbx_env_open(env, path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_RDONLY | MDBX_LIFORECLAIM, 0664));
  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(count_records(txn, items, &count));
  REQUIRE(count == 192, "fault seed record count changed");
  CHECK(expect_pattern(txn, items, 0, 0));
  CHECK(expect_pattern(txn, items, 31, 0));
  CHECK(expect_pattern(txn, items, 127, 0));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("fault seed verification", rc, __FILE__, __LINE__);
  goto bailout;
}

static int create_defrag_fault_seed(const char *path, const char *overflow_name, void *scratch) {
  enum { overflow_scratch_bytes = 12000 + 6 * 1024 };
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_txn *nested = NULL;
  MDBX_dbi overflow = 0;
  void *old_buffer = NULL;
  int rc;

  old_buffer = malloc(overflow_scratch_bytes);
  if (!old_buffer)
    return fail_msg("malloc failed", __FILE__, __LINE__);

  CHECK(delete_fault_path(path));
  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_maxdbs(env, 2));
  CHECK(mdbx_env_set_geometry(env, 0, 0, 16 * 1024 * 1024, 64 * 1024, 64 * 1024, -1));
  CHECK(mdbx_env_set_option(env, MDBX_opt_txn_dp_limit, 128));
  CHECK(mdbx_env_set_option(env, MDBX_opt_txn_dp_initial, 128));
  CHECK(mdbx_env_set_option(env, MDBX_opt_spill_min_denominator, 2));
  CHECK(mdbx_env_set_option(env, MDBX_opt_spill_max_denominator, 2));
  CHECK(mdbx_env_open(env, path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM, 0664));

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, overflow_name, MDBX_CREATE | MDBX_INTEGERKEY, &overflow));
  for (uint64_t key = 0; key < 64; ++key) {
    if (key % 9 == 0)
      CHECK(reserve_overflow_pattern(txn, overflow, key, 0));
    else
      CHECK(put_overflow_pattern(txn, overflow, key, 0, scratch));
  }
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, overflow_name, MDBX_DB_ACCEDE, &overflow));
  {
    const uint64_t key_value = 31;
    MDBX_val key = val((void *)&key_value, sizeof(key_value));
    MDBX_val new_data = val(scratch, overflow_value_size(key_value, 1));
    MDBX_val old_data = val(old_buffer, overflow_scratch_bytes);
    fill_pattern(scratch, new_data.iov_len, overflow_value_seed(key_value, 1));
    CHECK(mdbx_replace(txn, overflow, &key, &new_data, &old_data, 0));
    REQUIRE(old_data.iov_len == overflow_value_size(key_value, 0), "replace returned wrong old overflow size");
    REQUIRE(check_pattern(old_data.iov_base, old_data.iov_len, overflow_value_seed(key_value, 0)),
            "replace returned wrong old overflow bytes");
  }
  for (uint64_t key = 32; key < 48; ++key) {
    MDBX_val k = val(&key, sizeof(key));
    CHECK(mdbx_del(txn, overflow, &k, NULL));
  }
  for (uint64_t key = 64; key < 96; ++key)
    CHECK(put_overflow_pattern(txn, overflow, key, 2, scratch));

  CHECK(mdbx_txn_begin(env, txn, 0, &nested));
  CHECK(put_overflow_pattern(nested, overflow, 1, 2, scratch));
  CHECK(put_overflow_pattern(nested, overflow, 96, 2, scratch));
  CHECK(mdbx_txn_abort(nested));
  nested = NULL;
  CHECK(expect_overflow_pattern(txn, overflow, 1, 0));
  CHECK(expect_notfound(txn, overflow, 96));

  CHECK(mdbx_txn_begin(env, txn, 0, &nested));
  CHECK(put_overflow_pattern(nested, overflow, 2, 2, scratch));
  CHECK(put_overflow_pattern(nested, overflow, 96, 2, scratch));
  CHECK(mdbx_txn_commit(nested));
  nested = NULL;
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(verify_overflow_after_defrag(txn, overflow));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (nested)
    mdbx_txn_abort(nested);
  if (txn)
    mdbx_txn_abort(txn);
  if (overflow && env)
    mdbx_dbi_close(env, overflow);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  free(old_buffer);
  return rc;

bailout_rc:
  rc = fail_rc("defrag fault seed operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int verify_defrag_fault_seed(const char *path, const char *overflow_name) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi overflow = 0;
  int rc;

  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_maxdbs(env, 2));
  CHECK(mdbx_env_open(env, path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_RDONLY | MDBX_LIFORECLAIM, 0664));
  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_dbi_open(txn, overflow_name, MDBX_DB_ACCEDE, &overflow));
  CHECK(verify_overflow_after_defrag(txn, overflow));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (overflow && env)
    mdbx_dbi_close(env, overflow);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("defrag fault seed verification", rc, __FILE__, __LINE__);
  goto bailout;
}

static int verify_fault_recovery_write(const char *path, const char *items_name, void *scratch, uint64_t followup_key) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  int dead = -1;
  size_t count = 0;
  int rc;

  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_maxdbs(env, 2));
  CHECK(mdbx_env_open(env, path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM, 0664));
  CHECK(mdbx_reader_check(env, &dead));
  REQUIRE(dead >= 0, "reader-check returned an invalid dead-reader count after faulted child");

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(count_records(txn, items, &count));
  REQUIRE(count == 192, "fault recovery seed record count changed before follow-up write");
  CHECK(expect_pattern(txn, items, 0, 0));
  CHECK(expect_pattern(txn, items, 31, 0));
  CHECK(expect_pattern(txn, items, 127, 0));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(put_pattern(txn, items, followup_key, 4, scratch));
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(expect_pattern(txn, items, followup_key, 4));
  CHECK(count_records(txn, items, &count));
  REQUIRE(count == 193, "fault recovery follow-up write was not durable");
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("fault recovery write operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int expect_faulty_open(const char *path, const char *fault, int expected) {
  MDBX_env *env = NULL;
  int fault_set = 0;
  int rc;

  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_maxdbs(env, 2));
  CHECK(set_env_var("MDBX_TEST_DXB_FAULT", fault));
  fault_set = 1;
  const int open_rc =
      mdbx_env_open(env, path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_RDONLY | MDBX_LIFORECLAIM, 0664);
  CHECK(unset_env_var("MDBX_TEST_DXB_FAULT"));
  fault_set = 0;
  EXPECT_RC(open_rc, expected);

  rc = MDBX_SUCCESS;

bailout:
  if (fault_set)
    unset_env_var("MDBX_TEST_DXB_FAULT");
  if (env)
    mdbx_env_close(env);
  return rc;

bailout_rc:
  rc = fail_rc("faulty open operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int expect_faulty_read(const char *path, const char *items_name, const char *fault, int expected, uint64_t key) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  int fault_set = 0;
  int rc;

  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_maxdbs(env, 2));
  CHECK(mdbx_env_open(env, path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_RDONLY | MDBX_LIFORECLAIM, 0664));
  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));

  MDBX_val k = val(&key, sizeof(key));
  MDBX_val data = val(NULL, 0);
  CHECK(set_env_var("MDBX_TEST_DXB_FAULT", fault));
  fault_set = 1;
  const int get_rc = mdbx_get(txn, items, &k, &data);
  CHECK(unset_env_var("MDBX_TEST_DXB_FAULT"));
  fault_set = 0;
  EXPECT_RC(get_rc, expected);

  rc = MDBX_SUCCESS;

bailout:
  if (fault_set)
    unset_env_var("MDBX_TEST_DXB_FAULT");
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env)
    mdbx_env_close(env);
  return rc;

bailout_rc:
  rc = fail_rc("faulty read operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int expect_faulty_create(const char *path, const char *fault, int expected) {
  MDBX_env *env = NULL;
  int fault_set = 0;
  int rc;

  CHECK(delete_fault_path(path));
  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_maxdbs(env, 2));
  CHECK(mdbx_env_set_geometry(env, 0, 0, 16 * 1024 * 1024, 64 * 1024, 64 * 1024, -1));
  CHECK(set_env_var("MDBX_TEST_DXB_FAULT", fault));
  fault_set = 1;
  const int open_rc = mdbx_env_open(env, path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM, 0664);
  CHECK(unset_env_var("MDBX_TEST_DXB_FAULT"));
  fault_set = 0;
  EXPECT_RC(open_rc, expected);

  rc = MDBX_SUCCESS;

bailout:
  if (fault_set)
    unset_env_var("MDBX_TEST_DXB_FAULT");
  if (env)
    mdbx_env_close(env);
  {
    const int delete_rc = delete_fault_path(path);
    if (rc == MDBX_SUCCESS)
      rc = delete_rc;
  }
  return rc;

bailout_rc:
  rc = fail_rc("faulty create operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int expect_faulty_growth(const char *path, const char *items_name, const char *fault, int expected,
                                uint64_t key_base, size_t records, void *scratch) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  int fault_set = 0;
  int rc;

  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_maxdbs(env, 2));
  CHECK(mdbx_env_set_option(env, MDBX_opt_txn_dp_limit, 128));
  CHECK(mdbx_env_set_option(env, MDBX_opt_txn_dp_initial, 128));
  CHECK(mdbx_env_set_option(env, MDBX_opt_spill_min_denominator, 2));
  CHECK(mdbx_env_set_option(env, MDBX_opt_spill_max_denominator, 2));
  CHECK(mdbx_env_open(env, path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM, 0664));
  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));

  CHECK(set_env_var("MDBX_TEST_DXB_FAULT", fault));
  fault_set = 1;
  int put_rc = MDBX_SUCCESS;
  for (uint64_t key = key_base; key < key_base + records; ++key) {
    put_rc = put_pattern(txn, items, key, 1, scratch);
    if (put_rc != MDBX_SUCCESS)
      break;
  }
  CHECK(unset_env_var("MDBX_TEST_DXB_FAULT"));
  fault_set = 0;
  EXPECT_RC(put_rc, expected);

  rc = MDBX_SUCCESS;

bailout:
  if (fault_set)
    unset_env_var("MDBX_TEST_DXB_FAULT");
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env)
    mdbx_env_close(env);
  if (rc == MDBX_SUCCESS)
    rc = verify_fault_seed(path, items_name);
  return rc;

bailout_rc:
  rc = fail_rc("faulty growth operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int expect_faulty_defrag(const char *path, const char *overflow_name, const char *fault, int expected) {
  MDBX_env *env = NULL;
  int fault_set = 0;
  int rc;

  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_maxdbs(env, 2));
  CHECK(mdbx_env_set_option(env, MDBX_opt_txn_dp_limit, 128));
  CHECK(mdbx_env_set_option(env, MDBX_opt_txn_dp_initial, 128));
  CHECK(mdbx_env_set_option(env, MDBX_opt_spill_min_denominator, 2));
  CHECK(mdbx_env_set_option(env, MDBX_opt_spill_max_denominator, 2));
  CHECK(mdbx_env_open(env, path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM, 0664));

  MDBX_defrag_result_t result;
  memset(&result, 0, sizeof(result));
  CHECK(set_env_var("MDBX_TEST_DXB_FAULT", fault));
  fault_set = 1;
  const int defrag_rc = mdbx_env_defrag(env, 0, 0, 0, 0, -1, 8, NULL, NULL, &result);
  CHECK(unset_env_var("MDBX_TEST_DXB_FAULT"));
  fault_set = 0;
  EXPECT_RC(defrag_rc, expected);
  REQUIRE((result.stopping_reasons & MDBX_defrag_error) != 0, "faulted defrag did not report an error stop reason");

  rc = MDBX_SUCCESS;

bailout:
  if (fault_set)
    unset_env_var("MDBX_TEST_DXB_FAULT");
  if (env)
    mdbx_env_close(env);
  if (rc == MDBX_SUCCESS)
    rc = verify_defrag_fault_seed(path, overflow_name);
  return rc;

bailout_rc:
  rc = fail_rc("faulty defrag operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int expect_faulty_commit(const char *path, const char *items_name, const char *fault, const char *write_order,
                                int expected, uint64_t key_base, size_t records, void *scratch) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  int fault_set = 0;
  int order_set = 0;
  int rc;

  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_maxdbs(env, 2));
  CHECK(mdbx_env_set_option(env, MDBX_opt_txn_dp_limit, 128));
  CHECK(mdbx_env_set_option(env, MDBX_opt_txn_dp_initial, 128));
  CHECK(mdbx_env_set_option(env, MDBX_opt_spill_min_denominator, 2));
  CHECK(mdbx_env_set_option(env, MDBX_opt_spill_max_denominator, 2));
  CHECK(mdbx_env_open(env, path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM, 0664));
  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  for (uint64_t key = key_base; key < key_base + records; ++key)
    CHECK(put_pattern(txn, items, key, 1, scratch));

  if (write_order) {
    CHECK(set_env_var("MDBX_TEST_DXB_WRITE_ORDER", write_order));
    order_set = 1;
  }
  CHECK(set_env_var("MDBX_TEST_DXB_FAULT", fault));
  fault_set = 1;
  const int commit_rc = mdbx_txn_commit(txn);
  txn = NULL;
  CHECK(unset_env_var("MDBX_TEST_DXB_FAULT"));
  fault_set = 0;
  if (order_set) {
    CHECK(unset_env_var("MDBX_TEST_DXB_WRITE_ORDER"));
    order_set = 0;
  }
  EXPECT_RC(commit_rc, expected);

  rc = MDBX_SUCCESS;

bailout:
  if (fault_set)
    unset_env_var("MDBX_TEST_DXB_FAULT");
  if (order_set)
    unset_env_var("MDBX_TEST_DXB_WRITE_ORDER");
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env)
    mdbx_env_close(env);
  if (rc == MDBX_SUCCESS)
    rc = verify_fault_seed(path, items_name);
  return rc;

bailout_rc:
  rc = fail_rc("faulty commit operation", rc, __FILE__, __LINE__);
  goto bailout;
}

#if !defined(_WIN32) && !defined(_WIN64)
static int wait_fault_child_success(pid_t child) {
  int status = 0;
  pid_t waited;

  do {
    waited = waitpid(child, &status, 0);
  } while (waited < 0 && errno == EINTR);

  if (waited < 0)
    return fail_errno("waitpid", errno, __FILE__, __LINE__);
  if (WIFEXITED(status) && WEXITSTATUS(status) == EXIT_SUCCESS)
    return MDBX_SUCCESS;

  fprintf(stderr, "%s:%d: fault child process failed: status=0x%x\n", __FILE__, __LINE__, status);
  return MDBX_PROBLEM;
}

static int fault_child_commit_without_cleanup(const char *path, const char *items_name, const char *fault,
                                              const char *write_order, int expected, uint64_t key_base, size_t records,
                                              void *scratch) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  int fault_set = 0;
  int order_set = 0;
  int rc;

  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_maxdbs(env, 2));
  CHECK(mdbx_env_set_option(env, MDBX_opt_txn_dp_limit, 128));
  CHECK(mdbx_env_set_option(env, MDBX_opt_txn_dp_initial, 128));
  CHECK(mdbx_env_set_option(env, MDBX_opt_spill_min_denominator, 2));
  CHECK(mdbx_env_set_option(env, MDBX_opt_spill_max_denominator, 2));
  CHECK(mdbx_env_open(env, path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM, 0664));
  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  for (uint64_t key = key_base; key < key_base + records; ++key)
    CHECK(put_pattern(txn, items, key, 1, scratch));

  if (write_order) {
    CHECK(set_env_var("MDBX_TEST_DXB_WRITE_ORDER", write_order));
    order_set = 1;
  }
  CHECK(set_env_var("MDBX_TEST_DXB_FAULT", fault));
  fault_set = 1;
  const int commit_rc = mdbx_txn_commit(txn);
  txn = NULL;
  EXPECT_RC(commit_rc, expected);

  return MDBX_SUCCESS;

bailout:
  if (fault_set)
    unset_env_var("MDBX_TEST_DXB_FAULT");
  if (order_set)
    unset_env_var("MDBX_TEST_DXB_WRITE_ORDER");
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env)
    mdbx_env_close(env);
  return rc;

bailout_rc:
  rc = fail_rc("fault child commit operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int expect_faulty_commit_crash_restart(const char *path, const char *items_name, const char *fault,
                                              const char *write_order, int expected, uint64_t key_base, size_t records,
                                              uint64_t followup_key, void *scratch) {
  pid_t child;
  int rc;

  child = fork();
  if (child < 0)
    return fail_errno("fork", errno, __FILE__, __LINE__);

  if (child == 0) {
    const int child_rc =
        fault_child_commit_without_cleanup(path, items_name, fault, write_order, expected, key_base, records, scratch);
    _exit(child_rc == MDBX_SUCCESS ? EXIT_SUCCESS : EXIT_FAILURE);
  }

  CHECK(wait_fault_child_success(child));
  child = -1;
  CHECK(verify_fault_recovery_write(path, items_name, scratch, followup_key));
  return MDBX_SUCCESS;

bailout:
  if (child > 0) {
    int wait_rc = wait_fault_child_success(child);
    if (rc == MDBX_SUCCESS)
      rc = wait_rc;
  }
  return rc;

bailout_rc:
  rc = fail_rc("fault crash/restart operation", rc, __FILE__, __LINE__);
  goto bailout;
}
#endif /* !Windows */

static int exercise_fault_injection(void) {
  static const struct {
    const char *name;
    const char *fault;
    int expected;
  } open_cases[] = {
      {"filesize-eio", "filesize:EIO", MDBX_EIO},
      {"filesize-eintr", "filesize:EINTR", MDBX_EINTR},
      {"filesize-complete-eio", "filesize-complete:EIO", MDBX_EIO},
      {"filesize-complete-cancel", "filesize-complete:CANCEL", MDBX_EINTR},
      {"read-eio", "read:EIO", MDBX_EIO},
      {"read-short", "read:SHORT", MDBX_ENODATA},
      {"read-eintr", "read:EINTR", MDBX_EINTR},
      {"read-complete-eio", "read-complete:EIO", MDBX_EIO},
      {"read-complete-cancel", "read-complete:CANCEL", MDBX_EINTR},
  };
  static const struct {
    const char *name;
    const char *fault;
    int expected;
    uint64_t key;
  } read_cases[] = {
      {"read-complete-eio", "read-complete:EIO", MDBX_EIO, 31},
      {"read-complete-cancel", "read-complete:CANCEL", MDBX_EINTR, 31},
      {"read-complete-eio-after-one", "read-complete:EIO@1", MDBX_EIO, 31},
  };
  static const struct {
    const char *name;
    const char *fault;
    int expected;
  } create_cases[] = {
      {"setsize-enospc", "setsize:ENOSPC", MDBX_TEST_ENOSPC},
      {"setsize-complete-eio", "setsize-complete:EIO", MDBX_EIO},
      {"setsize-complete-cancel", "setsize-complete:CANCEL", MDBX_EINTR},
  };
  static const struct {
    const char *name;
    const char *fault;
    int expected;
    size_t records;
  } growth_cases[] = {
      {"setsize-complete-eio-growth", "setsize-complete:EIO", MDBX_EIO, 4096},
      {"setsize-complete-cancel-growth", "setsize-complete:CANCEL", MDBX_EINTR, 4096},
  };
#if defined(__linux__)
  static const struct {
    const char *name;
    const char *fault;
    int expected;
  } defrag_cases[] = {
      {"copy-eio-defrag", "copy:EIO", MDBX_EIO},
      {"copy-cancel-defrag", "copy:CANCEL", MDBX_EINTR},
      {"copy-complete-eio-defrag", "copy-complete:EIO", MDBX_EIO},
      {"copy-complete-cancel-defrag", "copy-complete:CANCEL", MDBX_EINTR},
  };
#endif /* Linux */
  static const struct {
    const char *name;
    const char *fault;
    const char *write_order;
    int expected;
    size_t records;
  } commit_cases[] = {
      {"writev-eio", "writev:EIO", NULL, MDBX_EIO, 512},
      {"writev-short", "writev:SHORT", NULL, MDBX_EIO, 512},
      {"writev-eio-after-one", "writev:EIO@1", NULL, MDBX_EIO, 512},
      {"writev-enospc-after-one", "writev:ENOSPC@1", NULL, MDBX_TEST_ENOSPC, 512},
      {"writev-cancel-after-one", "writev:CANCEL@1", NULL, MDBX_EINTR, 512},
      {"writev-eio-after-one-reverse", "writev:EIO@1", "reverse", MDBX_EIO, 512},
      {"writev-enospc-after-one-reverse", "writev:ENOSPC@1", "reverse", MDBX_TEST_ENOSPC, 512},
      {"writev-cancel-after-one-reverse", "writev:CANCEL@1", "reverse", MDBX_EINTR, 512},
      {"writev-eio-after-one-outside-in", "writev:EIO@1", "outside-in", MDBX_EIO, 512},
      {"writev-enospc-after-one-outside-in", "writev:ENOSPC@1", "outside-in", MDBX_TEST_ENOSPC, 512},
      {"writev-cancel-after-one-outside-in", "writev:CANCEL@1", "outside-in", MDBX_EINTR, 512},
      {"writev-partial-eio", "writev-partial:EIO", NULL, MDBX_EIO, 512},
      {"writev-partial-eio-after-one", "writev-partial:EIO@1", NULL, MDBX_EIO, 512},
      {"writev-partial-enospc-after-one", "writev-partial:ENOSPC@1", NULL, MDBX_TEST_ENOSPC, 512},
      {"writev-partial-cancel-after-one", "writev-partial:CANCEL@1", NULL, MDBX_EINTR, 512},
      {"writev-partial-eio-after-one-reverse", "writev-partial:EIO@1", "reverse", MDBX_EIO, 512},
      {"writev-partial-enospc-after-one-reverse", "writev-partial:ENOSPC@1", "reverse", MDBX_TEST_ENOSPC, 512},
      {"writev-partial-cancel-after-one-reverse", "writev-partial:CANCEL@1", "reverse", MDBX_EINTR, 512},
      {"writev-partial-eio-after-one-outside-in", "writev-partial:EIO@1", "outside-in", MDBX_EIO, 512},
      {"writev-partial-enospc-after-one-outside-in", "writev-partial:ENOSPC@1", "outside-in",
       MDBX_TEST_ENOSPC, 512},
      {"writev-partial-cancel-after-one-outside-in", "writev-partial:CANCEL@1", "outside-in", MDBX_EINTR, 512},
      {"writev-complete-eio", "writev-complete:EIO", NULL, MDBX_EIO, 512},
      {"writev-complete-cancel", "writev-complete:CANCEL", NULL, MDBX_EINTR, 512},
      {"writev-complete-eio-after-one-outside-in", "writev-complete:EIO@1", "outside-in", MDBX_EIO, 512},
      {"writev-complete-cancel-after-one-outside-in", "writev-complete:CANCEL@1", "outside-in", MDBX_EINTR, 512},
      {"write-complete-eio", "write-complete:EIO", NULL, MDBX_EIO, 512},
      {"write-complete-cancel", "write-complete:CANCEL", NULL, MDBX_EINTR, 512},
      {"sync-eio", "sync:EIO", NULL, MDBX_EIO, 512},
      {"sync-cancel", "sync:CANCEL", NULL, MDBX_EINTR, 512},
      {"sync-complete-eio", "sync-complete:EIO", NULL, MDBX_EIO, 512},
      {"sync-complete-cancel", "sync-complete:CANCEL", NULL, MDBX_EINTR, 512},
      {"write-enospc", "write:ENOSPC", NULL, MDBX_TEST_ENOSPC, 512},
      {"write-eintr", "write:EINTR", NULL, MDBX_EINTR, 512},
  };
#if !defined(_WIN32) && !defined(_WIN64)
  static const struct {
    const char *name;
    const char *fault;
    const char *write_order;
    int expected;
    size_t records;
  } crash_cases[] = {
      {"writev-eio-after-one-crash", "writev:EIO@1", NULL, MDBX_EIO, 512},
      {"writev-enospc-after-one-crash", "writev:ENOSPC@1", NULL, MDBX_TEST_ENOSPC, 512},
      {"writev-cancel-after-one-crash", "writev:CANCEL@1", NULL, MDBX_EINTR, 512},
      {"writev-eio-after-one-reverse-crash", "writev:EIO@1", "reverse", MDBX_EIO, 512},
      {"writev-cancel-after-one-reverse-crash", "writev:CANCEL@1", "reverse", MDBX_EINTR, 512},
      {"writev-eio-after-one-outside-in-crash", "writev:EIO@1", "outside-in", MDBX_EIO, 512},
      {"writev-cancel-after-one-outside-in-crash", "writev:CANCEL@1", "outside-in", MDBX_EINTR, 512},
      {"writev-partial-eio-crash", "writev-partial:EIO", NULL, MDBX_EIO, 512},
      {"writev-partial-enospc-after-one-crash", "writev-partial:ENOSPC@1", NULL, MDBX_TEST_ENOSPC, 512},
      {"writev-partial-cancel-after-one-crash", "writev-partial:CANCEL@1", NULL, MDBX_EINTR, 512},
      {"writev-partial-enospc-after-one-reverse-crash", "writev-partial:ENOSPC@1", "reverse",
       MDBX_TEST_ENOSPC, 512},
      {"writev-partial-cancel-after-one-reverse-crash", "writev-partial:CANCEL@1", "reverse", MDBX_EINTR, 512},
      {"writev-partial-eio-after-one-outside-in-crash", "writev-partial:EIO@1", "outside-in", MDBX_EIO, 512},
      {"writev-partial-cancel-after-one-outside-in-crash", "writev-partial:CANCEL@1", "outside-in", MDBX_EINTR,
       512},
      {"writev-complete-eio-crash", "writev-complete:EIO", NULL, MDBX_EIO, 512},
      {"writev-complete-cancel-crash", "writev-complete:CANCEL", NULL, MDBX_EINTR, 512},
      {"writev-complete-eio-after-one-outside-in-crash", "writev-complete:EIO@1", "outside-in", MDBX_EIO, 512},
      {"write-complete-eio-crash", "write-complete:EIO", NULL, MDBX_EIO, 512},
      {"sync-eio-crash", "sync:EIO", NULL, MDBX_EIO, 512},
      {"sync-complete-eio-crash", "sync-complete:EIO", NULL, MDBX_EIO, 512},
  };
#endif /* !Windows */
  const char *items_name = "items";
  const char *overflow_name = "overflow";
  char path[160];
  void *scratch = NULL;
  int rc;

  scratch = malloc(12000 + 6 * 1024);
  if (!scratch)
    return fail_msg("malloc failed", __FILE__, __LINE__);

  CHECK(unset_env_var("MDBX_TEST_DXB_FAULT"));
  CHECK(unset_env_var("MDBX_TEST_DXB_WRITE_ORDER"));
  CHECK(set_env_var("MDBX_FORCE_NO_DATA_MMAP", "1"));

  for (size_t i = 0; i < ARRAY_LENGTH(open_cases); ++i) {
    snprintf(path, sizeof(path), "./migration-smoke-%lu-fault-%s", smoke_run_id(), open_cases[i].name);
    CHECK(create_fault_seed(path, items_name, scratch));
    CHECK(expect_faulty_open(path, open_cases[i].fault, open_cases[i].expected));
    CHECK(verify_fault_seed(path, items_name));
    CHECK(delete_fault_path(path));
  }

  for (size_t i = 0; i < ARRAY_LENGTH(read_cases); ++i) {
    snprintf(path, sizeof(path), "./migration-smoke-%lu-fault-%s", smoke_run_id(), read_cases[i].name);
    CHECK(create_fault_seed(path, items_name, scratch));
    CHECK(expect_faulty_read(path, items_name, read_cases[i].fault, read_cases[i].expected, read_cases[i].key));
    CHECK(verify_fault_seed(path, items_name));
    CHECK(delete_fault_path(path));
  }

  for (size_t i = 0; i < ARRAY_LENGTH(create_cases); ++i) {
    snprintf(path, sizeof(path), "./migration-smoke-%lu-fault-%s", smoke_run_id(), create_cases[i].name);
    CHECK(expect_faulty_create(path, create_cases[i].fault, create_cases[i].expected));
  }

  for (size_t i = 0; i < ARRAY_LENGTH(growth_cases); ++i) {
    snprintf(path, sizeof(path), "./migration-smoke-%lu-fault-%s", smoke_run_id(), growth_cases[i].name);
    CHECK(create_fault_seed(path, items_name, scratch));
    CHECK(expect_faulty_growth(path, items_name, growth_cases[i].fault, growth_cases[i].expected,
                               500000 + (uint64_t)i * 100000, growth_cases[i].records, scratch));
    CHECK(delete_fault_path(path));
  }

#if defined(__linux__)
  for (size_t i = 0; i < ARRAY_LENGTH(defrag_cases); ++i) {
    snprintf(path, sizeof(path), "./migration-smoke-%lu-fault-%s", smoke_run_id(), defrag_cases[i].name);
    CHECK(create_defrag_fault_seed(path, overflow_name, scratch));
    CHECK(expect_faulty_defrag(path, overflow_name, defrag_cases[i].fault, defrag_cases[i].expected));
    CHECK(delete_fault_path(path));
  }
#endif /* Linux */

  for (size_t i = 0; i < ARRAY_LENGTH(commit_cases); ++i) {
    snprintf(path, sizeof(path), "./migration-smoke-%lu-fault-%s", smoke_run_id(), commit_cases[i].name);
    CHECK(create_fault_seed(path, items_name, scratch));
    CHECK(expect_faulty_commit(path, items_name, commit_cases[i].fault, commit_cases[i].write_order,
                               commit_cases[i].expected,
                               1000 + (uint64_t)i * 10000, commit_cases[i].records, scratch));
    CHECK(delete_fault_path(path));
  }

#if !defined(_WIN32) && !defined(_WIN64)
  for (size_t i = 0; i < ARRAY_LENGTH(crash_cases); ++i) {
    snprintf(path, sizeof(path), "./migration-smoke-%lu-fault-%s", smoke_run_id(), crash_cases[i].name);
    CHECK(create_fault_seed(path, items_name, scratch));
    CHECK(expect_faulty_commit_crash_restart(path, items_name, crash_cases[i].fault, crash_cases[i].write_order,
                                             crash_cases[i].expected, 100000 + (uint64_t)i * 10000, crash_cases[i].records,
                                             120000 + (uint64_t)i, scratch));
    CHECK(delete_fault_path(path));
  }
#endif /* !Windows */

  rc = MDBX_SUCCESS;

bailout:
  unset_env_var("MDBX_TEST_DXB_FAULT");
  unset_env_var("MDBX_TEST_DXB_WRITE_ORDER");
  free(scratch);
  return rc;

bailout_rc:
  rc = fail_rc("fault injection operation", rc, __FILE__, __LINE__);
  goto bailout;
}

#endif /* MIGRATION_FAULT_INJECTION */

#if defined(_WIN32) || defined(_WIN64)
static int exercise_multiprocess_reader(void) { return MDBX_SUCCESS; }
static int exercise_multiprocess_stress(void) { return MDBX_SUCCESS; }
static int exercise_multiprocess_crash_stress(void) { return MDBX_SUCCESS; }
static int exercise_process_crash_restart(void) { return MDBX_SUCCESS; }
#else

#define CHECK_SYS(expr)                                                                                                \
  do {                                                                                                                 \
    if ((expr) != 0) {                                                                                                 \
      rc = fail_errno(#expr, errno, __FILE__, __LINE__);                                                               \
      goto bailout;                                                                                                    \
    }                                                                                                                  \
  } while (0)

static int pipe_write_byte(int fd, char byte) {
  for (;;) {
    const ssize_t written = write(fd, &byte, 1);
    if (written == 1)
      return MDBX_SUCCESS;
    if (written < 0 && errno == EINTR)
      continue;
    if (written < 0)
      return fail_errno("write(pipe)", errno, __FILE__, __LINE__);
    return fail_msg("write(pipe) wrote zero bytes", __FILE__, __LINE__);
  }
}

static int pipe_read_byte(int fd, char *byte) {
  for (;;) {
    const ssize_t got = read(fd, byte, 1);
    if (got == 1)
      return MDBX_SUCCESS;
    if (got < 0 && errno == EINTR)
      continue;
    if (got < 0)
      return fail_errno("read(pipe)", errno, __FILE__, __LINE__);
    return fail_msg("read(pipe) reached EOF", __FILE__, __LINE__);
  }
}

static int wait_child_success(pid_t child) {
  int status = 0;
  pid_t waited;

  do {
    waited = waitpid(child, &status, 0);
  } while (waited < 0 && errno == EINTR);

  if (waited < 0)
    return fail_errno("waitpid", errno, __FILE__, __LINE__);
  if (WIFEXITED(status) && WEXITSTATUS(status) == EXIT_SUCCESS)
    return MDBX_SUCCESS;

  fprintf(stderr, "%s:%d: child process failed: status=0x%x\n", __FILE__, __LINE__, status);
  return MDBX_PROBLEM;
}

static int wait_child_signal(pid_t child, int expected_signal) {
  int status = 0;
  pid_t waited;

  do {
    waited = waitpid(child, &status, 0);
  } while (waited < 0 && errno == EINTR);

  if (waited < 0)
    return fail_errno("waitpid", errno, __FILE__, __LINE__);
  if (WIFSIGNALED(status) && WTERMSIG(status) == expected_signal)
    return MDBX_SUCCESS;

  fprintf(stderr, "%s:%d: child process ended unexpectedly: status=0x%x, expected signal %d\n", __FILE__, __LINE__,
          status, expected_signal);
  return MDBX_PROBLEM;
}

static int close_pipe_fd(int *fd) {
  int rc = MDBX_SUCCESS;
  if (*fd >= 0) {
    while (close(*fd) != 0) {
      if (errno == EINTR)
        continue;
      rc = fail_errno("close(pipe)", errno, __FILE__, __LINE__);
      break;
    }
    *fd = -1;
  }
  return rc;
}

static int open_multiprocess_env_ex(const char *path, MDBX_env_flags_t flags, int force_spill, MDBX_env **out) {
  int rc;
  MDBX_env *env = NULL;

  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_maxdbs(env, 4));
  CHECK(mdbx_env_set_geometry(env, 0, 0, 32 * 1024 * 1024, 64 * 1024, 64 * 1024, -1));
  if (force_spill) {
    CHECK(mdbx_env_set_option(env, MDBX_opt_txn_dp_limit, 128));
    CHECK(mdbx_env_set_option(env, MDBX_opt_txn_dp_initial, 128));
    CHECK(mdbx_env_set_option(env, MDBX_opt_spill_min_denominator, 2));
    CHECK(mdbx_env_set_option(env, MDBX_opt_spill_max_denominator, 2));
  }
  CHECK(mdbx_env_open(env, path, flags, 0664));

  *out = env;
  return MDBX_SUCCESS;

bailout:
  if (env)
    mdbx_env_close(env);
  return rc;

bailout_rc:
  rc = fail_rc("multiprocess env open", rc, __FILE__, __LINE__);
  goto bailout;
}

static int open_multiprocess_env(const char *path, MDBX_env_flags_t flags, MDBX_env **out) {
  return open_multiprocess_env_ex(path, flags, 0, out);
}

#ifndef MULTIPROCESS_STRESS_READERS
#define MULTIPROCESS_STRESS_READERS 2
#endif
#ifndef MULTIPROCESS_STRESS_WRITERS
#define MULTIPROCESS_STRESS_WRITERS 2
#endif
#ifndef MULTIPROCESS_STRESS_WAVES
#define MULTIPROCESS_STRESS_WAVES 4
#endif
#ifndef MULTIPROCESS_STRESS_RANDOMIZED
#define MULTIPROCESS_STRESS_RANDOMIZED 0
#endif
#ifndef MULTIPROCESS_CRASH_STRESS
#define MULTIPROCESS_CRASH_STRESS 0
#endif

#if MULTIPROCESS_STRESS_READERS < 1 || MULTIPROCESS_STRESS_READERS > 8
#error "MULTIPROCESS_STRESS_READERS must be in the range 1..8"
#endif
#if MULTIPROCESS_STRESS_WRITERS < 1 || MULTIPROCESS_STRESS_WRITERS > 4
#error "MULTIPROCESS_STRESS_WRITERS must be in the range 1..4"
#endif
#if MULTIPROCESS_STRESS_WAVES < 1 || MULTIPROCESS_STRESS_WAVES > 8
#error "MULTIPROCESS_STRESS_WAVES must be in the range 1..8"
#endif
#if MULTIPROCESS_STRESS_RANDOMIZED != 0 && MULTIPROCESS_STRESS_RANDOMIZED != 1
#error "MULTIPROCESS_STRESS_RANDOMIZED must be 0 or 1"
#endif
#if MULTIPROCESS_CRASH_STRESS != 0 && MULTIPROCESS_CRASH_STRESS != 1
#error "MULTIPROCESS_CRASH_STRESS must be 0 or 1"
#endif

static unsigned stress_wave_slot(unsigned writer_id, unsigned wave) {
#if MULTIPROCESS_STRESS_RANDOMIZED
  static const unsigned order[8] = {5, 2, 7, 1, 6, 0, 4, 3};
  const unsigned order_length = (unsigned)ARRAY_LENGTH(order);
  unsigned seen = 0;

  for (unsigned i = 0; i < order_length; ++i) {
    const unsigned candidate = (order[(i + writer_id * 3u) % order_length] + writer_id) % order_length;
    if (candidate < MULTIPROCESS_STRESS_WAVES) {
      if (seen == wave)
        return candidate;
      ++seen;
    }
  }
#else
  (void)writer_id;
#endif
  return wave;
}

static uint64_t stress_update_key(unsigned writer_id, unsigned wave) {
  return 8 + writer_id * 16 + stress_wave_slot(writer_id, wave);
}

static uint64_t stress_delete_key(unsigned writer_id, unsigned wave) {
  return 72 + writer_id * 16 + stress_wave_slot(writer_id, wave);
}

static uint64_t stress_insert_key(unsigned writer_id, unsigned wave) {
  return 8060 + writer_id * 128 + stress_wave_slot(writer_id, wave);
}

static unsigned stress_generation(unsigned writer_id, unsigned wave) { return 20 + writer_id * 10 + wave; }

static uint64_t stress_overflow_update_key(unsigned writer_id, unsigned wave) {
  return 1000 + writer_id * 64 + stress_wave_slot(writer_id, wave);
}

static uint64_t stress_overflow_delete_key(unsigned writer_id, unsigned wave) {
  return 2000 + writer_id * 64 + stress_wave_slot(writer_id, wave);
}

static uint64_t stress_overflow_insert_key(unsigned writer_id, unsigned wave) {
  return 3000 + writer_id * 64 + stress_wave_slot(writer_id, wave);
}

static size_t stress_overflow_entry_count(void) {
  return (size_t)MULTIPROCESS_STRESS_WRITERS * (size_t)MULTIPROCESS_STRESS_WAVES * 2;
}

static int create_multiprocess_seed(const char *path, const char *items_name, MDBX_env_flags_t flags, void *scratch) {
  const char *overflow_name = "stress-overflow";
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  MDBX_dbi overflow = 0;
  int rc;

  CHECK(open_multiprocess_env(path, flags, &env));
  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_CREATE | MDBX_INTEGERKEY, &items));
  CHECK(mdbx_dbi_open(txn, overflow_name, MDBX_CREATE | MDBX_INTEGERKEY, &overflow));
  for (uint64_t i = 0; i < 128; ++i)
    CHECK(put_pattern(txn, items, i, 0, scratch));
  for (unsigned writer_id = 0; writer_id < MULTIPROCESS_STRESS_WRITERS; ++writer_id) {
    for (unsigned wave = 0; wave < MULTIPROCESS_STRESS_WAVES; ++wave) {
      CHECK(put_overflow_pattern(txn, overflow, stress_overflow_update_key(writer_id, wave), 0, scratch));
      CHECK(put_overflow_pattern(txn, overflow, stress_overflow_delete_key(writer_id, wave), 0, scratch));
    }
  }
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (overflow && env)
    mdbx_dbi_close(env, overflow);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("multiprocess seed operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int multiprocess_writer_update(const char *path, const char *items_name, void *scratch) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  int rc;

  CHECK(open_multiprocess_env(path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM, &env));
  CHECK(grow_active_geometry(env, 512 * 1024, "multiprocess writer did not grow geometry"));
  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(put_pattern(txn, items, 17, 1, scratch));
  CHECK(put_pattern(txn, items, 5000, 0, scratch));
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  CHECK(grow_active_geometry(env, 256 * 1024, "multiprocess writer did not grow geometry on second wave"));
  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(put_pattern(txn, items, 64, 2, scratch));
  {
    uint64_t deleted = 7;
    MDBX_val deleted_key = val(&deleted, sizeof(deleted));
    CHECK(mdbx_del(txn, items, &deleted_key, NULL));
  }
  CHECK(put_pattern(txn, items, 6000, 0, scratch));
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("multiprocess writer operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int multiprocess_reader_child(const char *path, const char *items_name, int ready_fd, int committed_fd) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  char signal = 0;
  int rc;

  CHECK(open_multiprocess_env(path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_RDONLY | MDBX_LIFORECLAIM, &env));
  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(expect_pattern(txn, items, 17, 0));
  CHECK(pipe_write_byte(ready_fd, 'r'));
  CHECK(pipe_read_byte(committed_fd, &signal));
  REQUIRE(signal == 'c', "unexpected parent-to-child sync byte");

  CHECK(expect_pattern(txn, items, 17, 0));
  CHECK(expect_pattern(txn, items, 64, 0));
  CHECK(expect_pattern(txn, items, 7, 0));
  CHECK(expect_notfound(txn, items, 5000));
  CHECK(expect_notfound(txn, items, 6000));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(check_env_info_snapshot(env, txn));
  CHECK(expect_pattern(txn, items, 17, 1));
  CHECK(expect_pattern(txn, items, 5000, 0));
  CHECK(expect_pattern(txn, items, 64, 2));
  CHECK(expect_notfound(txn, items, 7));
  CHECK(expect_pattern(txn, items, 6000, 0));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("multiprocess reader operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int exercise_multiprocess_reader(void) {
  char path[128];
  const char *items_name = "items";
  int child_ready[2] = {-1, -1};
  int parent_committed[2] = {-1, -1};
  pid_t child = -1;
  void *scratch = NULL;
  char signal = 0;
  int rc;

  snprintf(path, sizeof(path), "./migration-smoke-%lu-multiprocess", smoke_run_id());
  rc = mdbx_env_delete(path, MDBX_ENV_JUST_DELETE);
  if (rc != MDBX_SUCCESS && rc != MDBX_RESULT_TRUE)
    return fail_rc("mdbx_env_delete", rc, __FILE__, __LINE__);

  scratch = malloc(12000 + 6 * 1024);
  if (!scratch)
    return fail_msg("malloc failed", __FILE__, __LINE__);

  CHECK(create_multiprocess_seed(path, items_name, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM, scratch));
  CHECK_SYS(pipe(child_ready));
  CHECK_SYS(pipe(parent_committed));

  child = fork();
  if (child < 0) {
    rc = fail_errno("fork", errno, __FILE__, __LINE__);
    goto bailout;
  }

  if (child == 0) {
    (void)close(child_ready[0]);
    (void)close(parent_committed[1]);
    const int child_rc = multiprocess_reader_child(path, items_name, child_ready[1], parent_committed[0]);
    (void)close(child_ready[1]);
    (void)close(parent_committed[0]);
    _exit(child_rc == MDBX_SUCCESS ? EXIT_SUCCESS : EXIT_FAILURE);
  }

  CHECK(close_pipe_fd(&child_ready[1]));
  CHECK(close_pipe_fd(&parent_committed[0]));
  CHECK(pipe_read_byte(child_ready[0], &signal));
  REQUIRE(signal == 'r', "unexpected child-to-parent sync byte");
  CHECK(multiprocess_writer_update(path, items_name, scratch));
  CHECK(pipe_write_byte(parent_committed[1], 'c'));
  CHECK(close_pipe_fd(&parent_committed[1]));
  CHECK(wait_child_success(child));
  child = -1;

  rc = MDBX_SUCCESS;

bailout:
  if (child_ready[0] >= 0 && rc == MDBX_SUCCESS)
    rc = close_pipe_fd(&child_ready[0]);
  else
    (void)close_pipe_fd(&child_ready[0]);
  (void)close_pipe_fd(&child_ready[1]);
  (void)close_pipe_fd(&parent_committed[0]);
  (void)close_pipe_fd(&parent_committed[1]);
  if (child > 0) {
    int wait_rc = wait_child_success(child);
    if (rc == MDBX_SUCCESS)
      rc = wait_rc;
  }
  free(scratch);
  if (rc == MDBX_SUCCESS) {
    int delete_rc = mdbx_env_delete(path, MDBX_ENV_JUST_DELETE);
    if (delete_rc != MDBX_SUCCESS && delete_rc != MDBX_RESULT_TRUE)
      rc = fail_rc("mdbx_env_delete cleanup", delete_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("multiprocess operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int multiprocess_stress_verify_overflow(MDBX_txn *txn, MDBX_dbi overflow, int final_snapshot) {
  int rc;

  CHECK(check_overflow_stat(txn, overflow, stress_overflow_entry_count()));
  for (unsigned writer_id = 0; writer_id < MULTIPROCESS_STRESS_WRITERS; ++writer_id) {
    for (unsigned wave = 0; wave < MULTIPROCESS_STRESS_WAVES; ++wave) {
      const unsigned generation = stress_generation(writer_id, wave);
      if (final_snapshot) {
        CHECK(expect_overflow_pattern(txn, overflow, stress_overflow_update_key(writer_id, wave), generation));
        CHECK(expect_notfound(txn, overflow, stress_overflow_delete_key(writer_id, wave)));
        CHECK(expect_overflow_pattern(txn, overflow, stress_overflow_insert_key(writer_id, wave), generation));
      } else {
        CHECK(expect_overflow_pattern(txn, overflow, stress_overflow_update_key(writer_id, wave), 0));
        CHECK(expect_overflow_pattern(txn, overflow, stress_overflow_delete_key(writer_id, wave), 0));
        CHECK(expect_notfound(txn, overflow, stress_overflow_insert_key(writer_id, wave)));
      }
    }
  }

  return MDBX_SUCCESS;

bailout_rc:
  rc = fail_rc("multiprocess stress overflow snapshot verification", rc, __FILE__, __LINE__);
  return rc;
}

static int multiprocess_stress_verify(MDBX_txn *txn, MDBX_dbi items, MDBX_dbi overflow, int final_snapshot) {
  int rc;

  for (unsigned writer_id = 0; writer_id < MULTIPROCESS_STRESS_WRITERS; ++writer_id) {
    for (unsigned wave = 0; wave < MULTIPROCESS_STRESS_WAVES; ++wave) {
      const unsigned generation = stress_generation(writer_id, wave);
      if (final_snapshot) {
        CHECK(expect_pattern(txn, items, stress_update_key(writer_id, wave), generation));
        CHECK(expect_notfound(txn, items, stress_delete_key(writer_id, wave)));
        CHECK(expect_pattern(txn, items, stress_insert_key(writer_id, wave), generation));
      } else {
        CHECK(expect_pattern(txn, items, stress_update_key(writer_id, wave), 0));
        CHECK(expect_pattern(txn, items, stress_delete_key(writer_id, wave), 0));
        CHECK(expect_notfound(txn, items, stress_insert_key(writer_id, wave)));
      }
    }
  }
  CHECK(multiprocess_stress_verify_overflow(txn, overflow, final_snapshot));

  return MDBX_SUCCESS;

bailout_rc:
  rc = fail_rc("multiprocess stress snapshot verification", rc, __FILE__, __LINE__);
  return rc;
}

static int multiprocess_stress_reader_child(const char *path, const char *items_name, unsigned reader_id,
                                            int ready_fd, int release_fd) {
  const char *overflow_name = "stress-overflow";
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  MDBX_dbi overflow = 0;
  char signal = 0;
  int rc;

  CHECK(open_multiprocess_env(path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_RDONLY | MDBX_LIFORECLAIM, &env));
  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(mdbx_dbi_open(txn, overflow_name, MDBX_DB_ACCEDE, &overflow));
  CHECK(expect_pattern(txn, items, reader_id, 0));
  CHECK(multiprocess_stress_verify(txn, items, overflow, 0));
  CHECK(pipe_write_byte(ready_fd, 'r'));
  CHECK(pipe_read_byte(release_fd, &signal));
  REQUIRE(signal == 'c', "unexpected multiprocess stress release byte");

  CHECK(multiprocess_stress_verify(txn, items, overflow, 0));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(check_env_info_snapshot(env, txn));
  CHECK(multiprocess_stress_verify(txn, items, overflow, 1));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (overflow && env)
    mdbx_dbi_close(env, overflow);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("multiprocess stress reader operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int multiprocess_stress_writer_child(const char *path, const char *items_name, unsigned writer_id,
                                            void *scratch) {
  const char *overflow_name = "stress-overflow";
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  MDBX_dbi overflow = 0;
  int rc;

  CHECK(open_multiprocess_env_ex(path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM, 1, &env));
  if (writer_id == 0)
    CHECK(grow_active_geometry(env, 512 * 1024, "multiprocess stress writer did not grow geometry"));

  for (unsigned wave = 0; wave < MULTIPROCESS_STRESS_WAVES; ++wave) {
    const unsigned generation = stress_generation(writer_id, wave);
    CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
    CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
    CHECK(mdbx_dbi_open(txn, overflow_name, MDBX_DB_ACCEDE, &overflow));
    CHECK(put_pattern(txn, items, stress_update_key(writer_id, wave), generation, scratch));
    {
      uint64_t deleted = stress_delete_key(writer_id, wave);
      MDBX_val deleted_key = val(&deleted, sizeof(deleted));
      CHECK(mdbx_del(txn, items, &deleted_key, NULL));
    }
    CHECK(put_pattern(txn, items, stress_insert_key(writer_id, wave), generation, scratch));
    if ((writer_id + wave) % 2)
      CHECK(reserve_overflow_pattern(txn, overflow, stress_overflow_update_key(writer_id, wave), generation));
    else
      CHECK(put_overflow_pattern(txn, overflow, stress_overflow_update_key(writer_id, wave), generation, scratch));
    {
      uint64_t deleted = stress_overflow_delete_key(writer_id, wave);
      MDBX_val deleted_key = val(&deleted, sizeof(deleted));
      CHECK(mdbx_del(txn, overflow, &deleted_key, NULL));
    }
    CHECK(put_overflow_pattern(txn, overflow, stress_overflow_insert_key(writer_id, wave), generation, scratch));
    CHECK(mdbx_txn_commit(txn));
    txn = NULL;
  }

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (overflow && env)
    mdbx_dbi_close(env, overflow);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("multiprocess stress writer operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int verify_multiprocess_stress_final(const char *path, const char *items_name, void *scratch) {
  const char *overflow_name = "stress-overflow";
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  MDBX_dbi overflow = 0;
  size_t count = 0;
  int rc;

  CHECK(open_multiprocess_env(path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM, &env));
  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(mdbx_dbi_open(txn, overflow_name, MDBX_DB_ACCEDE, &overflow));
  CHECK(multiprocess_stress_verify(txn, items, overflow, 1));
  CHECK(count_records(txn, items, &count));
  REQUIRE(count == 128, "unexpected multiprocess stress final count");
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(mdbx_dbi_open(txn, overflow_name, MDBX_DB_ACCEDE, &overflow));
  CHECK(put_pattern(txn, items, 9001, 4, scratch));
  CHECK(put_overflow_pattern(txn, overflow, 9001, 4, scratch));
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(expect_pattern(txn, items, 9001, 4));
  CHECK(expect_overflow_pattern(txn, overflow, 9001, 4));
  CHECK(check_overflow_stat(txn, overflow, stress_overflow_entry_count() + 1));
  CHECK(count_records(txn, items, &count));
  REQUIRE(count == 129, "unexpected multiprocess stress post-write count");
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (overflow && env)
    mdbx_dbi_close(env, overflow);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("multiprocess stress final verification", rc, __FILE__, __LINE__);
  goto bailout;
}

static int exercise_multiprocess_stress(void) {
  char path[128];
  const char *items_name = "items";
  int reader_ready[MULTIPROCESS_STRESS_READERS][2];
  int reader_release[MULTIPROCESS_STRESS_READERS][2];
  pid_t readers[MULTIPROCESS_STRESS_READERS];
  pid_t writers[MULTIPROCESS_STRESS_WRITERS];
  void *scratch = NULL;
  int rc;

  for (unsigned i = 0; i < MULTIPROCESS_STRESS_READERS; ++i) {
    reader_ready[i][0] = -1;
    reader_ready[i][1] = -1;
    reader_release[i][0] = -1;
    reader_release[i][1] = -1;
    readers[i] = -1;
  }
  for (unsigned i = 0; i < MULTIPROCESS_STRESS_WRITERS; ++i)
    writers[i] = -1;

  snprintf(path, sizeof(path), "./migration-smoke-%lu-multiprocess-stress", smoke_run_id());
  rc = mdbx_env_delete(path, MDBX_ENV_JUST_DELETE);
  if (rc != MDBX_SUCCESS && rc != MDBX_RESULT_TRUE)
    return fail_rc("mdbx_env_delete", rc, __FILE__, __LINE__);

  scratch = malloc(12000 + 6 * 1024);
  if (!scratch)
    return fail_msg("malloc failed", __FILE__, __LINE__);

  CHECK(create_multiprocess_seed(path, items_name, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM, scratch));

  for (unsigned i = 0; i < MULTIPROCESS_STRESS_READERS; ++i) {
    CHECK_SYS(pipe(reader_ready[i]));
    CHECK_SYS(pipe(reader_release[i]));
    readers[i] = fork();
    if (readers[i] < 0) {
      rc = fail_errno("fork", errno, __FILE__, __LINE__);
      goto bailout;
    }
    if (readers[i] == 0) {
      for (unsigned fdset = 0; fdset < MULTIPROCESS_STRESS_READERS; ++fdset) {
        if (fdset != i) {
          if (reader_ready[fdset][0] >= 0)
            (void)close(reader_ready[fdset][0]);
          if (reader_ready[fdset][1] >= 0)
            (void)close(reader_ready[fdset][1]);
          if (reader_release[fdset][0] >= 0)
            (void)close(reader_release[fdset][0]);
          if (reader_release[fdset][1] >= 0)
            (void)close(reader_release[fdset][1]);
        }
      }
      (void)close(reader_ready[i][0]);
      (void)close(reader_release[i][1]);
      const int child_rc =
          multiprocess_stress_reader_child(path, items_name, i, reader_ready[i][1], reader_release[i][0]);
      (void)close(reader_ready[i][1]);
      (void)close(reader_release[i][0]);
      _exit(child_rc == MDBX_SUCCESS ? EXIT_SUCCESS : EXIT_FAILURE);
    }
    CHECK(close_pipe_fd(&reader_ready[i][1]));
    CHECK(close_pipe_fd(&reader_release[i][0]));
  }

  for (unsigned i = 0; i < MULTIPROCESS_STRESS_READERS; ++i) {
    char signal = 0;
    CHECK(pipe_read_byte(reader_ready[i][0], &signal));
    REQUIRE(signal == 'r', "unexpected multiprocess stress ready byte");
  }

  {
    MDBX_env *observer = NULL;
    CHECK(open_multiprocess_env(path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM, &observer));
    CHECK(check_reader_list_snapshot(observer, MULTIPROCESS_STRESS_READERS, 0));
    CHECK(mdbx_env_close(observer));
  }

  for (unsigned i = 0; i < MULTIPROCESS_STRESS_WRITERS; ++i) {
    writers[i] = fork();
    if (writers[i] < 0) {
      rc = fail_errno("fork", errno, __FILE__, __LINE__);
      goto bailout;
    }
    if (writers[i] == 0) {
      for (unsigned fdset = 0; fdset < MULTIPROCESS_STRESS_READERS; ++fdset) {
        if (reader_ready[fdset][0] >= 0)
          (void)close(reader_ready[fdset][0]);
        if (reader_ready[fdset][1] >= 0)
          (void)close(reader_ready[fdset][1]);
        if (reader_release[fdset][0] >= 0)
          (void)close(reader_release[fdset][0]);
        if (reader_release[fdset][1] >= 0)
          (void)close(reader_release[fdset][1]);
      }
      const int child_rc = multiprocess_stress_writer_child(path, items_name, i, scratch);
      _exit(child_rc == MDBX_SUCCESS ? EXIT_SUCCESS : EXIT_FAILURE);
    }
  }

  for (unsigned i = 0; i < MULTIPROCESS_STRESS_WRITERS; ++i) {
    CHECK(wait_child_success(writers[i]));
    writers[i] = -1;
  }

  {
    MDBX_env *observer = NULL;
    CHECK(open_multiprocess_env(path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM, &observer));
    CHECK(check_reader_list_snapshot(observer, MULTIPROCESS_STRESS_READERS, 1));
    CHECK(mdbx_env_close(observer));
  }

  for (unsigned i = 0; i < MULTIPROCESS_STRESS_READERS; ++i) {
    CHECK(pipe_write_byte(reader_release[i][1], 'c'));
    CHECK(close_pipe_fd(&reader_release[i][1]));
  }

  for (unsigned i = 0; i < MULTIPROCESS_STRESS_READERS; ++i) {
    CHECK(wait_child_success(readers[i]));
    readers[i] = -1;
  }

  CHECK(verify_multiprocess_stress_final(path, items_name, scratch));
  rc = MDBX_SUCCESS;

bailout:
  for (unsigned i = 0; i < MULTIPROCESS_STRESS_READERS; ++i) {
    (void)close_pipe_fd(&reader_ready[i][0]);
    (void)close_pipe_fd(&reader_ready[i][1]);
    (void)close_pipe_fd(&reader_release[i][0]);
    (void)close_pipe_fd(&reader_release[i][1]);
  }
  for (unsigned i = 0; i < MULTIPROCESS_STRESS_WRITERS; ++i) {
    if (writers[i] > 0) {
      int wait_rc = wait_child_success(writers[i]);
      if (rc == MDBX_SUCCESS)
        rc = wait_rc;
    }
  }
  for (unsigned i = 0; i < MULTIPROCESS_STRESS_READERS; ++i) {
    if (readers[i] > 0) {
      int wait_rc = wait_child_success(readers[i]);
      if (rc == MDBX_SUCCESS)
        rc = wait_rc;
    }
  }
  free(scratch);
  if (rc == MDBX_SUCCESS) {
    int delete_rc = mdbx_env_delete(path, MDBX_ENV_JUST_DELETE);
    if (delete_rc != MDBX_SUCCESS && delete_rc != MDBX_RESULT_TRUE)
      rc = fail_rc("mdbx_env_delete cleanup", delete_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("multiprocess stress operation", rc, __FILE__, __LINE__);
  goto bailout;
}

typedef int (*crash_child_func)(const char *path, const char *items_name, MDBX_env_flags_t flags, void *scratch);

static int process_crash_committed_child(const char *path, const char *items_name, MDBX_env_flags_t flags,
                                         void *scratch) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  int rc;

  CHECK(open_multiprocess_env(path, flags, &env));
  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(put_pattern(txn, items, 7000, 1, scratch));
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  return MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env)
    mdbx_env_close(env);
  return rc;

bailout_rc:
  rc = fail_rc("process-crash committed writer operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int process_crash_abandoned_child(const char *path, const char *items_name, MDBX_env_flags_t flags,
                                         void *scratch) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  int rc;

  CHECK(open_multiprocess_env_ex(path, flags, 1, &env));
  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  for (uint64_t key = 7100; key < 7350; ++key)
    CHECK(put_pattern(txn, items, key, 2, scratch));

  return MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env)
    mdbx_env_close(env);
  return rc;

bailout_rc:
  rc = fail_rc("process-crash abandoned writer operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int fork_crash_child(crash_child_func child_func, const char *path, const char *items_name,
                            MDBX_env_flags_t flags, void *scratch) {
  int rc;
  pid_t child = fork();
  if (child < 0)
    return fail_errno("fork", errno, __FILE__, __LINE__);

  if (child == 0) {
    const int child_rc = child_func(path, items_name, flags, scratch);
    _exit(child_rc == MDBX_SUCCESS ? EXIT_SUCCESS : EXIT_FAILURE);
  }

  CHECK(wait_child_success(child));
  return MDBX_SUCCESS;

bailout_rc:
  rc = fail_rc("process-crash child operation", rc, __FILE__, __LINE__);
  return rc;
}

typedef int (*kill_crash_child_func)(const char *path, const char *items_name, MDBX_env_flags_t flags, void *scratch,
                                     int ready_fd);

static int process_kill_committed_child(const char *path, const char *items_name, MDBX_env_flags_t flags,
                                        void *scratch, int ready_fd) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  int rc;

  CHECK(open_multiprocess_env(path, flags, &env));
  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(put_pattern(txn, items, 7500, 4, scratch));
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;
  CHECK(pipe_write_byte(ready_fd, 'k'));

  for (;;)
    pause();

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env)
    mdbx_env_close(env);
  return rc;

bailout_rc:
  rc = fail_rc("process-kill committed writer operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int process_kill_dirty_child(const char *path, const char *items_name, MDBX_env_flags_t flags, void *scratch,
                                    int ready_fd) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  int rc;

  CHECK(open_multiprocess_env_ex(path, flags, 1, &env));
  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  for (uint64_t key = 7600; key < 7850; ++key)
    CHECK(put_pattern(txn, items, key, 5, scratch));
  CHECK(pipe_write_byte(ready_fd, 'd'));

  for (;;)
    pause();

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env)
    mdbx_env_close(env);
  return rc;

bailout_rc:
  rc = fail_rc("process-kill dirty writer operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int fork_killed_crash_child(kill_crash_child_func child_func, const char *path, const char *items_name,
                                   MDBX_env_flags_t flags, void *scratch, char expected_ready) {
  int ready[2] = {-1, -1};
  char signal = 0;
  pid_t child = -1;
  int rc;

  CHECK_SYS(pipe(ready));
  child = fork();
  if (child < 0) {
    rc = fail_errno("fork", errno, __FILE__, __LINE__);
    goto bailout;
  }

  if (child == 0) {
    (void)close(ready[0]);
    const int child_rc = child_func(path, items_name, flags, scratch, ready[1]);
    (void)close(ready[1]);
    _exit(child_rc == MDBX_SUCCESS ? EXIT_SUCCESS : EXIT_FAILURE);
  }

  CHECK(close_pipe_fd(&ready[1]));
  CHECK(pipe_read_byte(ready[0], &signal));
  REQUIRE(signal == expected_ready, "unexpected process-kill ready byte");
  CHECK(close_pipe_fd(&ready[0]));
  CHECK_SYS(kill(child, SIGKILL));
  CHECK(wait_child_signal(child, SIGKILL));
  child = -1;

  return MDBX_SUCCESS;

bailout:
  (void)close_pipe_fd(&ready[0]);
  (void)close_pipe_fd(&ready[1]);
  if (child > 0) {
    (void)kill(child, SIGKILL);
    int wait_rc = wait_child_signal(child, SIGKILL);
    if (rc == MDBX_SUCCESS)
      rc = wait_rc;
  }
  return rc;

bailout_rc:
  rc = fail_rc("process-kill child operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int verify_committed_child_restart(const char *path, const char *items_name, MDBX_env_flags_t flags) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  int dead = -1;
  int rc;

  CHECK(open_multiprocess_env(path, flags, &env));
  CHECK(mdbx_reader_check(env, &dead));
  REQUIRE(dead >= 0, "reader-check returned an invalid dead-reader count");
  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(expect_pattern(txn, items, 7000, 1));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("process-crash committed restart verification", rc, __FILE__, __LINE__);
  goto bailout;
}

static int verify_abandoned_writer_recovery(const char *path, const char *items_name, MDBX_env_flags_t flags,
                                            void *scratch) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  int dead = -1;
  size_t count = 0;
  int rc;

  CHECK(open_multiprocess_env(path, flags, &env));
  CHECK(mdbx_reader_check(env, &dead));
  REQUIRE(dead >= 0, "reader-check returned an invalid dead-reader count after abandoned writer");

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(expect_pattern(txn, items, 7000, 1));
  for (uint64_t key = 7100; key < 7350; key += 31)
    CHECK(expect_notfound(txn, items, key));
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(put_pattern(txn, items, 7400, 3, scratch));
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(expect_pattern(txn, items, 7000, 1));
  CHECK(expect_pattern(txn, items, 7400, 3));
  CHECK(expect_notfound(txn, items, 7100));
  CHECK(count_records(txn, items, &count));
  REQUIRE(count == 130, "unexpected record count after abandoned writer recovery");
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("process-crash abandoned writer verification", rc, __FILE__, __LINE__);
  goto bailout;
}

static int verify_killed_committed_restart(const char *path, const char *items_name, MDBX_env_flags_t flags) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  int dead = -1;
  size_t count = 0;
  int rc;

  CHECK(open_multiprocess_env(path, flags, &env));
  CHECK(mdbx_reader_check(env, &dead));
  REQUIRE(dead >= 0, "reader-check returned an invalid dead-reader count after killed commit");
  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(expect_pattern(txn, items, 7000, 1));
  CHECK(expect_pattern(txn, items, 7400, 3));
  CHECK(expect_pattern(txn, items, 7500, 4));
  CHECK(count_records(txn, items, &count));
  REQUIRE(count == 131, "unexpected record count after killed committed writer");
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("process-kill committed restart verification", rc, __FILE__, __LINE__);
  goto bailout;
}

static int verify_killed_dirty_recovery(const char *path, const char *items_name, MDBX_env_flags_t flags,
                                        void *scratch) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  int dead = -1;
  size_t count = 0;
  int rc;

  CHECK(open_multiprocess_env(path, flags, &env));
  CHECK(mdbx_reader_check(env, &dead));
  REQUIRE(dead >= 0, "reader-check returned an invalid dead-reader count after killed dirty writer");

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(expect_pattern(txn, items, 7000, 1));
  CHECK(expect_pattern(txn, items, 7400, 3));
  CHECK(expect_pattern(txn, items, 7500, 4));
  for (uint64_t key = 7600; key < 7850; key += 31)
    CHECK(expect_notfound(txn, items, key));
  CHECK(count_records(txn, items, &count));
  REQUIRE(count == 131, "unexpected record count after killed dirty writer");
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(put_pattern(txn, items, 7900, 6, scratch));
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(expect_pattern(txn, items, 7000, 1));
  CHECK(expect_pattern(txn, items, 7400, 3));
  CHECK(expect_pattern(txn, items, 7500, 4));
  CHECK(expect_pattern(txn, items, 7900, 6));
  CHECK(expect_notfound(txn, items, 7600));
  CHECK(count_records(txn, items, &count));
  REQUIRE(count == 132, "unexpected record count after killed dirty writer recovery");
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("process-kill dirty writer verification", rc, __FILE__, __LINE__);
  goto bailout;
}

#if MULTIPROCESS_CRASH_STRESS
static int multiprocess_crash_reader_child(const char *path, const char *items_name, unsigned reader_id,
                                           int ready_fd, int release_fd) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  char signal = 0;
  size_t count = 0;
  int rc;

  CHECK(open_multiprocess_env(path, MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_RDONLY | MDBX_LIFORECLAIM, &env));
  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(expect_pattern(txn, items, reader_id, 0));
  CHECK(expect_notfound(txn, items, 7000));
  CHECK(expect_notfound(txn, items, 7400));
  CHECK(count_records(txn, items, &count));
  REQUIRE(count == 128, "unexpected pinned multiprocess crash-stress count");
  CHECK(pipe_write_byte(ready_fd, 'r'));
  CHECK(pipe_read_byte(release_fd, &signal));
  REQUIRE(signal == 'c', "unexpected multiprocess crash-stress release byte");

  CHECK(expect_notfound(txn, items, 7000));
  CHECK(expect_notfound(txn, items, 7100));
  CHECK(expect_notfound(txn, items, 7400));
  CHECK(count_records(txn, items, &count));
  REQUIRE(count == 128, "pinned multiprocess crash-stress snapshot changed");
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(check_env_info_snapshot(env, txn));
  CHECK(expect_pattern(txn, items, 7000, 1));
  CHECK(expect_notfound(txn, items, 7100));
  CHECK(expect_notfound(txn, items, 7200));
  CHECK(expect_pattern(txn, items, 7400, 3));
  CHECK(count_records(txn, items, &count));
  REQUIRE(count == 130, "unexpected renewed multiprocess crash-stress count");
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("multiprocess crash-stress reader operation", rc, __FILE__, __LINE__);
  goto bailout;
}

static int verify_multiprocess_crash_stress_final(const char *path, const char *items_name, MDBX_env_flags_t flags,
                                                  void *scratch) {
  MDBX_env *env = NULL;
  MDBX_txn *txn = NULL;
  MDBX_dbi items = 0;
  size_t count = 0;
  int rc;

  CHECK(open_multiprocess_env(path, flags, &env));
  CHECK(mdbx_reader_check(env, NULL));
  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(expect_pattern(txn, items, 7000, 1));
  CHECK(expect_notfound(txn, items, 7100));
  CHECK(expect_pattern(txn, items, 7400, 3));
  CHECK(count_records(txn, items, &count));
  REQUIRE(count == 130, "unexpected multiprocess crash-stress final count");
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  CHECK(mdbx_dbi_open(txn, items_name, MDBX_DB_ACCEDE, &items));
  CHECK(put_pattern(txn, items, 7450, 4, scratch));
  CHECK(mdbx_txn_commit(txn));
  txn = NULL;

  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  CHECK(expect_pattern(txn, items, 7450, 4));
  CHECK(count_records(txn, items, &count));
  REQUIRE(count == 131, "unexpected multiprocess crash-stress post-write count");
  CHECK(mdbx_txn_abort(txn));
  txn = NULL;

  rc = MDBX_SUCCESS;

bailout:
  if (txn)
    mdbx_txn_abort(txn);
  if (items && env)
    mdbx_dbi_close(env, items);
  if (env) {
    int close_rc = mdbx_env_close(env);
    if (rc == MDBX_SUCCESS && close_rc != MDBX_SUCCESS)
      rc = fail_rc("mdbx_env_close", close_rc, __FILE__, __LINE__);
  }
  return rc;

bailout_rc:
  rc = fail_rc("multiprocess crash-stress final verification", rc, __FILE__, __LINE__);
  goto bailout;
}

static int run_multiprocess_crash_stress_case(const char *path, const char *items_name, MDBX_env_flags_t flags,
                                              void *scratch) {
  int reader_ready[MULTIPROCESS_STRESS_READERS][2];
  int reader_release[MULTIPROCESS_STRESS_READERS][2];
  pid_t readers[MULTIPROCESS_STRESS_READERS];
  int rc;

  for (unsigned i = 0; i < MULTIPROCESS_STRESS_READERS; ++i) {
    reader_ready[i][0] = -1;
    reader_ready[i][1] = -1;
    reader_release[i][0] = -1;
    reader_release[i][1] = -1;
    readers[i] = -1;
  }

  CHECK(create_multiprocess_seed(path, items_name, flags, scratch));

  for (unsigned i = 0; i < MULTIPROCESS_STRESS_READERS; ++i) {
    CHECK_SYS(pipe(reader_ready[i]));
    CHECK_SYS(pipe(reader_release[i]));
    readers[i] = fork();
    if (readers[i] < 0) {
      rc = fail_errno("fork", errno, __FILE__, __LINE__);
      goto bailout;
    }
    if (readers[i] == 0) {
      for (unsigned fdset = 0; fdset < MULTIPROCESS_STRESS_READERS; ++fdset) {
        if (fdset != i) {
          if (reader_ready[fdset][0] >= 0)
            (void)close(reader_ready[fdset][0]);
          if (reader_ready[fdset][1] >= 0)
            (void)close(reader_ready[fdset][1]);
          if (reader_release[fdset][0] >= 0)
            (void)close(reader_release[fdset][0]);
          if (reader_release[fdset][1] >= 0)
            (void)close(reader_release[fdset][1]);
        }
      }
      (void)close(reader_ready[i][0]);
      (void)close(reader_release[i][1]);
      const int child_rc =
          multiprocess_crash_reader_child(path, items_name, i, reader_ready[i][1], reader_release[i][0]);
      (void)close(reader_ready[i][1]);
      (void)close(reader_release[i][0]);
      _exit(child_rc == MDBX_SUCCESS ? EXIT_SUCCESS : EXIT_FAILURE);
    }
    CHECK(close_pipe_fd(&reader_ready[i][1]));
    CHECK(close_pipe_fd(&reader_release[i][0]));
  }

  for (unsigned i = 0; i < MULTIPROCESS_STRESS_READERS; ++i) {
    char signal = 0;
    CHECK(pipe_read_byte(reader_ready[i][0], &signal));
    REQUIRE(signal == 'r', "unexpected multiprocess crash-stress ready byte");
  }

  {
    MDBX_env *observer = NULL;
    CHECK(open_multiprocess_env(path, flags, &observer));
    CHECK(check_reader_list_snapshot(observer, MULTIPROCESS_STRESS_READERS, 0));
    CHECK(mdbx_env_close(observer));
  }

  CHECK(fork_crash_child(process_crash_committed_child, path, items_name, flags, scratch));
  CHECK(verify_committed_child_restart(path, items_name, flags));

  {
    MDBX_env *observer = NULL;
    CHECK(open_multiprocess_env(path, flags, &observer));
    CHECK(check_reader_list_snapshot(observer, MULTIPROCESS_STRESS_READERS, 1));
    CHECK(mdbx_env_close(observer));
  }

  CHECK(fork_crash_child(process_crash_abandoned_child, path, items_name, flags, scratch));
  CHECK(verify_abandoned_writer_recovery(path, items_name, flags, scratch));

  {
    MDBX_env *observer = NULL;
    CHECK(open_multiprocess_env(path, flags, &observer));
    CHECK(check_reader_list_snapshot(observer, MULTIPROCESS_STRESS_READERS, 1));
    CHECK(mdbx_env_close(observer));
  }

  for (unsigned i = 0; i < MULTIPROCESS_STRESS_READERS; ++i) {
    CHECK(pipe_write_byte(reader_release[i][1], 'c'));
    CHECK(close_pipe_fd(&reader_release[i][1]));
  }

  for (unsigned i = 0; i < MULTIPROCESS_STRESS_READERS; ++i) {
    CHECK(wait_child_success(readers[i]));
    readers[i] = -1;
  }

  CHECK(verify_multiprocess_crash_stress_final(path, items_name, flags, scratch));
  rc = MDBX_SUCCESS;

bailout:
  for (unsigned i = 0; i < MULTIPROCESS_STRESS_READERS; ++i) {
    if (reader_release[i][1] >= 0) {
      (void)pipe_write_byte(reader_release[i][1], 'c');
      (void)close_pipe_fd(&reader_release[i][1]);
    }
    (void)close_pipe_fd(&reader_ready[i][0]);
    (void)close_pipe_fd(&reader_ready[i][1]);
    (void)close_pipe_fd(&reader_release[i][0]);
    (void)close_pipe_fd(&reader_release[i][1]);
  }
  for (unsigned i = 0; i < MULTIPROCESS_STRESS_READERS; ++i) {
    if (readers[i] > 0) {
      int wait_rc = wait_child_success(readers[i]);
      if (rc == MDBX_SUCCESS)
        rc = wait_rc;
    }
  }
  return rc;

bailout_rc:
  rc = fail_rc("multiprocess crash-stress case", rc, __FILE__, __LINE__);
  goto bailout;
}

static int exercise_multiprocess_crash_stress(void) {
  static const struct {
    const char *name;
    MDBX_env_flags_t flags;
  } cases[] = {
      {"multiprocess-crash-durable", MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM},
      {"multiprocess-crash-nometasync", MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM | MDBX_NOMETASYNC},
      {"multiprocess-crash-safe-nosync", MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM | MDBX_SAFE_NOSYNC},
      {"multiprocess-crash-utterly", MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM | MDBX_UTTERLY_NOSYNC},
  };
  const char *items_name = "items";
  void *scratch = NULL;
  int rc = MDBX_SUCCESS;

  scratch = malloc(12000 + 6 * 1024);
  if (!scratch)
    return fail_msg("malloc failed", __FILE__, __LINE__);

  for (size_t i = 0; i < ARRAY_LENGTH(cases); ++i) {
    char path[128];
    snprintf(path, sizeof(path), "./migration-smoke-%lu-%s", smoke_run_id(), cases[i].name);
    rc = mdbx_env_delete(path, MDBX_ENV_JUST_DELETE);
    if (rc != MDBX_SUCCESS && rc != MDBX_RESULT_TRUE)
      goto bailout_delete;

    CHECK(run_multiprocess_crash_stress_case(path, items_name, cases[i].flags, scratch));

    rc = mdbx_env_delete(path, MDBX_ENV_JUST_DELETE);
    if (rc != MDBX_SUCCESS && rc != MDBX_RESULT_TRUE)
      goto bailout_delete;
  }

bailout:
  free(scratch);
  return rc;

bailout_delete:
  rc = fail_rc("mdbx_env_delete", rc, __FILE__, __LINE__);
  goto bailout;

bailout_rc:
  rc = fail_rc("multiprocess crash-stress operation", rc, __FILE__, __LINE__);
  goto bailout;
}
#else
static int exercise_multiprocess_crash_stress(void) { return MDBX_SUCCESS; }
#endif /* MULTIPROCESS_CRASH_STRESS */

static int exercise_process_crash_restart(void) {
  static const struct {
    const char *name;
    MDBX_env_flags_t flags;
  } cases[] = {
      {"process-crash-durable", MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM},
      {"process-crash-nometasync", MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM | MDBX_NOMETASYNC},
      {"process-crash-safe-nosync", MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM | MDBX_SAFE_NOSYNC},
      {"process-crash-utterly", MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM | MDBX_UTTERLY_NOSYNC},
  };
  const char *items_name = "items";
  void *scratch = NULL;
  int rc = MDBX_SUCCESS;

  scratch = malloc(12000 + 6 * 1024);
  if (!scratch)
    return fail_msg("malloc failed", __FILE__, __LINE__);

  for (size_t i = 0; i < ARRAY_LENGTH(cases); ++i) {
    char path[128];
    snprintf(path, sizeof(path), "./migration-smoke-%lu-%s", smoke_run_id(), cases[i].name);
    rc = mdbx_env_delete(path, MDBX_ENV_JUST_DELETE);
    if (rc != MDBX_SUCCESS && rc != MDBX_RESULT_TRUE)
      goto bailout_delete;

    CHECK(create_multiprocess_seed(path, items_name, cases[i].flags, scratch));
    CHECK(fork_crash_child(process_crash_committed_child, path, items_name, cases[i].flags, scratch));
    CHECK(verify_committed_child_restart(path, items_name, cases[i].flags));
    CHECK(fork_crash_child(process_crash_abandoned_child, path, items_name, cases[i].flags, scratch));
    CHECK(verify_abandoned_writer_recovery(path, items_name, cases[i].flags, scratch));
    CHECK(fork_killed_crash_child(process_kill_committed_child, path, items_name, cases[i].flags, scratch, 'k'));
    CHECK(verify_killed_committed_restart(path, items_name, cases[i].flags));
    CHECK(fork_killed_crash_child(process_kill_dirty_child, path, items_name, cases[i].flags, scratch, 'd'));
    CHECK(verify_killed_dirty_recovery(path, items_name, cases[i].flags, scratch));

    rc = mdbx_env_delete(path, MDBX_ENV_JUST_DELETE);
    if (rc != MDBX_SUCCESS && rc != MDBX_RESULT_TRUE)
      goto bailout_delete;
  }

bailout:
  free(scratch);
  return rc;

bailout_delete:
  rc = fail_rc("mdbx_env_delete", rc, __FILE__, __LINE__);
  goto bailout;

bailout_rc:
  rc = fail_rc("process-crash restart operation", rc, __FILE__, __LINE__);
  goto bailout;
}

#endif

#if defined(MIGRATION_FAULT_INJECTION)
int main(void) {
  return exercise_fault_injection() == MDBX_SUCCESS ? EXIT_SUCCESS : EXIT_FAILURE;
}
#else
int main(void) {
  static const struct {
    const char *name;
    MDBX_env_flags_t flags;
    int writemap_optional;
  } cases[] = {
      {"file-durable", MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM, 0},
      {"file-validation", MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM | MDBX_VALIDATION, 0},
      {"file-nometasync", MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM | MDBX_NOMETASYNC, 0},
      {"file-safe-nosync", MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM | MDBX_SAFE_NOSYNC, 0},
      {"file-lazy", MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM | MDBX_SAFE_NOSYNC | MDBX_NOMETASYNC, 0},
      {"file-utterly", MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM | MDBX_UTTERLY_NOSYNC, 0},
      {"writemap-durable", MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM | MDBX_WRITEMAP, 1},
      {"writemap-utterly", MDBX_NOSUBDIR | MDBX_NOSTICKYTHREADS | MDBX_LIFORECLAIM | MDBX_WRITEMAP |
                              MDBX_UTTERLY_NOSYNC,
       1},
  };

  for (size_t i = 0; i < ARRAY_LENGTH(cases); ++i) {
    int rc = exercise_mode(cases[i].name, cases[i].flags, cases[i].writemap_optional);
    if (rc != MDBX_SUCCESS)
      return EXIT_FAILURE;
  }

  if (exercise_explicit_profile_compatibility() != MDBX_SUCCESS)
    return EXIT_FAILURE;
  if (exercise_accede_open_policy() != MDBX_SUCCESS)
    return EXIT_FAILURE;
  if (exercise_writemap_set_flags_policy() != MDBX_SUCCESS)
    return EXIT_FAILURE;
  if (exercise_legacy_mapasync_policy() != MDBX_SUCCESS)
    return EXIT_FAILURE;
  if (exercise_lckless_readonly_nommap() != MDBX_SUCCESS)
    return EXIT_FAILURE;

  if (exercise_multiprocess_reader() != MDBX_SUCCESS)
    return EXIT_FAILURE;
  if (exercise_multiprocess_stress() != MDBX_SUCCESS)
    return EXIT_FAILURE;
  if (exercise_multiprocess_crash_stress() != MDBX_SUCCESS)
    return EXIT_FAILURE;
  if (exercise_process_crash_restart() != MDBX_SUCCESS)
    return EXIT_FAILURE;

  return EXIT_SUCCESS;
}
#endif /* MIGRATION_FAULT_INJECTION */
