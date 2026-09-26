/* Deterministic one-shot allocation and sync failures against the C API.
 * Link with --wrap=malloc,--wrap=calloc,--wrap=realloc,--wrap=msync,
 * --wrap=fsync,--wrap=fdatasync. Usage: DIRECTORY create|write|merge|sync N
 * Exit 0 requires the old or atomically committed generation after reopen.
 */
#include "mdbx.h"
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

static long remaining = -1, calls;
static int sync_fault, injected;
static int fail_now(int syncing) {
  if (remaining < 0 || syncing != sync_fault) return 0;
  ++calls;
  if (--remaining != 0) return 0;
  remaining = -1;
  injected = 1;
  errno = syncing ? EIO : ENOMEM;
  return 1;
}
void *__real_malloc(size_t);
void *__real_calloc(size_t, size_t);
void *__real_realloc(void *, size_t);
int __real_msync(void *, size_t, int);
int __real_fsync(int);
int __real_fdatasync(int);
void *__wrap_malloc(size_t n) { return fail_now(0) ? NULL : __real_malloc(n); }
void *__wrap_calloc(size_t n, size_t s) { return fail_now(0) ? NULL : __real_calloc(n, s); }
void *__wrap_realloc(void *p, size_t n) { return fail_now(0) ? NULL : __real_realloc(p, n); }
int __wrap_msync(void *p, size_t n, int flags) { return fail_now(1) ? -1 : __real_msync(p, n, flags); }
int __wrap_fsync(int fd) { return fail_now(1) ? -1 : __real_fsync(fd); }
int __wrap_fdatasync(int fd) { return fail_now(1) ? -1 : __real_fdatasync(fd); }

#define CHECK(call) do { int r_ = (call); if (r_) { fprintf(stderr, "%s:%d %s: %d %s\n", __FILE__, __LINE__, #call, r_, mdbx_strerror(r_)); exit(1); } } while (0)

static MDBX_env *open_env(const char *path) {
  MDBX_env *env;
  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_maxdbs(env, 16));
  CHECK(mdbx_env_set_geometry(env, 0, -1, 256L << 20, -1, -1, 4096));
  CHECK(mdbx_env_open(env, path, MDBX_WRITEMAP | MDBX_NOSTICKYTHREADS, 0600));
  return env;
}

int main(int argc, char **argv) {
  if (argc != 4) return 2;
  const int merging = !strncmp(argv[2], "merge", 5);
  const uint32_t rows = merging ? 16384 : 1024;
  MDBX_env *env = open_env(argv[1]);
  MDBX_txn *parent, *children[4] = {NULL};
  MDBX_dbi dbis[4];
  MDBX_subtxn_spec_t specs[4];
  CHECK(mdbx_txn_begin(env, NULL, 0, &parent));
  for (int t = 0; t < 4; ++t) {
    char name[16];
    snprintf(name, sizeof(name), "t%d", t);
    CHECK(mdbx_dbi_open(parent, name, MDBX_CREATE, &dbis[t]));
    for (uint32_t row = 0; row < rows; ++row) {
      uint32_t bytes = __builtin_bswap32(row);
      char buffer[256] = {0};
      MDBX_val key = {&bytes, sizeof(bytes)}, value = {buffer, sizeof(buffer)};
      CHECK(mdbx_put(parent, dbis[t], &key, &value, MDBX_UPSERT));
    }
    specs[t] = (MDBX_subtxn_spec_t){dbis[t], 4096};
  }
  CHECK(mdbx_txn_commit(parent));
  CHECK(mdbx_txn_begin(env, NULL, 0, &parent));
  const long at = strtol(argv[3], NULL, 10);
  if (!strcmp(argv[2], "create")) remaining = at;
  int rc = mdbx_txn_create_subtxns(parent, specs, 4, children);
  remaining = -1;
  if (rc) goto abort_parent;
  if (!strcmp(argv[2], "write")) remaining = at;
  for (int t = 0; t < 4 && !rc; ++t) {
    for (uint32_t row = 0; row < rows && !rc; ++row) {
      uint32_t bytes = __builtin_bswap32(row);
      char buffer[512];
      memset(buffer, 1, sizeof(buffer));
      MDBX_val key = {&bytes, sizeof(bytes)}, value = {buffer, sizeof(buffer)};
      rc = mdbx_put(children[t], dbis[t], &key, &value, MDBX_UPSERT);
    }
  }
  remaining = -1;
  if (rc) goto abort_children;
  if (merging) remaining = at;
  for (int t = 0; t < 4 && !rc; ++t) {
    rc = mdbx_subtx_commit(children[t]);
    if (rc && !strcmp(argv[2], "merge-retry")) {
      fprintf(stderr, "retrying child %d after error %d\n", t, rc);
      remaining = -1;
      rc = mdbx_subtx_commit(children[t]);
    }
    if (!rc) children[t] = NULL;
  }
  remaining = -1;
  if (rc) goto abort_children;
  if (!strcmp(argv[2], "sync")) { sync_fault = 1; remaining = at; }
  rc = mdbx_txn_commit(parent);
  remaining = -1;
  parent = NULL;
  goto reopen;
abort_children:
  remaining = -1;
  for (int t = 0; t < 4; ++t)
    if (children[t]) CHECK(mdbx_subtx_abort(children[t]));
abort_parent:
  remaining = -1;
  CHECK(mdbx_txn_abort(parent));
reopen:
  /* Dont retry a failed sync or flush on close: preserve the observed outcome. */
  CHECK(mdbx_env_close_ex(env, true));
  env = open_env(argv[1]);
  MDBX_txn *reader;
  CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &reader));
  int generation = -1;
  for (int t = 0; t < 4; ++t) {
    char name[16];
    snprintf(name, sizeof(name), "t%d", t);
    CHECK(mdbx_dbi_open(reader, name, 0, &dbis[t]));
    for (uint32_t row = 0; row < rows; ++row) {
      uint32_t bytes = __builtin_bswap32(row);
      MDBX_val key = {&bytes, sizeof(bytes)}, value;
      CHECK(mdbx_get(reader, dbis[t], &key, &value));
      int got = ((unsigned char *)value.iov_base)[0];
      if (generation < 0) generation = got;
      if (got != generation || (got != 0 && got != 1) || value.iov_len != (got ? 512u : 256u)) abort();
      for (size_t i = 0; i < value.iov_len; ++i)
        if (((unsigned char *)value.iov_base)[i] != got) abort();
    }
  }
  if ((!rc && generation != 1) || (rc && strcmp(argv[2], "sync") && generation != 0)) abort();
  CHECK(mdbx_txn_abort(reader));
  CHECK(mdbx_env_close(env));
  printf("stage=%s at=%ld calls=%ld injected=%d rc=%d generation=%d atomic=yes\n", argv[2], at, calls, injected, rc, generation);
  return 0;
}
