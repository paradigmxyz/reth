/* Public-API integration regression. Compile like prefault_copy.c.
 * Syscall shims force cold residency and count successful mapped-source CoW
 * writes; allocation, reclamation, transaction and persistence paths are real. */
#define _GNU_SOURCE 1
#define MDBX_BUILD_FLAGS "prefault-copy integration regression"
#include <sys/mman.h>
#include <sys/types.h>
#include <unistd.h>
static ssize_t test_pwrite(int fd, const void *buf, size_t count, off_t offset);
static int test_mincore(void *address, size_t length, void *vector);
#define pwrite test_pwrite
#define mincore(...) test_mincore(__VA_ARGS__)
#include "../libmdbx/mdbx.c"
#undef pwrite
#undef mincore

static MDBX_env *watched_env;
static size_t source_writes;

static ssize_t test_pwrite(int fd, const void *buf, size_t count, off_t offset) {
  const bool mapped_source =
      watched_env && fd == watched_env->lazy_fd && count == watched_env->ps &&
      (uintptr_t)buf >= (uintptr_t)watched_env->dxb_mmap.base &&
      (uintptr_t)buf + count <= (uintptr_t)watched_env->dxb_mmap.base + watched_env->dxb_mmap.current &&
      ((const page_t *)buf)->pgno != bytes2pgno(watched_env, offset);
  const ssize_t written = pwrite(fd, buf, count, offset);
  if (mapped_source && written == (ssize_t)count)
    ++source_writes;
  return written;
}

static int test_mincore(void *address, size_t length, void *vector) {
  (void)address;
  memset(vector, 0, (length + globals.sys_pagesize - 1) / globals.sys_pagesize);
  return 0;
}

#define CHECK(expr)                                                                                                    \
  do {                                                                                                                 \
    if (!(expr)) {                                                                                                     \
      fprintf(stderr, "%s:%d: %s\n", __FILE__, __LINE__, #expr);                                                       \
      abort();                                                                                                         \
    }                                                                                                                  \
  } while (0)
#define OK(expr) CHECK((expr) == MDBX_SUCCESS)

enum { KEY_COUNT = 1024 };

static void encode_key(unsigned key, unsigned char bytes[4]) {
  bytes[0] = (unsigned char)(key >> 24);
  bytes[1] = (unsigned char)(key >> 16);
  bytes[2] = (unsigned char)(key >> 8);
  bytes[3] = (unsigned char)key;
}

static size_t value_size(unsigned key, unsigned pagesize) { return key % 37 == 0 ? pagesize + 19 : 80 + key % 3 * 17; }

static void fill_value(unsigned char *value, size_t size, unsigned generation, unsigned key) {
  for (size_t i = 0; i < size; ++i)
    value[i] = (unsigned char)(generation * 13 ^ key ^ i);
}

static bool deleted(unsigned generation, unsigned key) { return generation > 0 && key % 11 == generation % 11; }

static void update(MDBX_env *env, unsigned generation, unsigned pagesize, bool abort_txn) {
  MDBX_txn *txn;
  OK(mdbx_txn_begin(env, NULL, 0, &txn));
  MDBX_dbi db;
  OK(mdbx_dbi_open(txn, NULL, 0, &db));
  unsigned char *value = malloc(pagesize + 19);
  CHECK(value);
  for (unsigned key = 0; key < KEY_COUNT; ++key) {
    unsigned char key_bytes[4];
    encode_key(key, key_bytes);
    MDBX_val k = {key_bytes, sizeof(key_bytes)};
    if (deleted(generation, key)) {
      const int rc = mdbx_del(txn, db, &k, NULL);
      CHECK(rc == MDBX_SUCCESS || rc == MDBX_NOTFOUND);
    } else {
      const size_t size = value_size(key, pagesize);
      fill_value(value, size, generation, key);
      MDBX_val v = {value, size};
      OK(mdbx_put(txn, db, &k, &v, 0));
    }
  }
  free(value);
  if (abort_txn)
    OK(mdbx_txn_abort(txn));
  else
    OK(mdbx_txn_commit(txn));
}

static void verify(MDBX_txn *txn, unsigned generation, unsigned pagesize) {
  MDBX_dbi db;
  OK(mdbx_dbi_open(txn, NULL, 0, &db));
  unsigned char *expected = malloc(pagesize + 19);
  CHECK(expected);
  unsigned present = 0;
  for (unsigned key = 0; key < KEY_COUNT; ++key) {
    unsigned char key_bytes[4];
    encode_key(key, key_bytes);
    MDBX_val k = {key_bytes, sizeof(key_bytes)}, v;
    const int rc = mdbx_get(txn, db, &k, &v);
    if (deleted(generation, key)) {
      CHECK(rc == MDBX_NOTFOUND);
    } else {
      OK(rc);
      const size_t size = value_size(key, pagesize);
      fill_value(expected, size, generation, key);
      CHECK(v.iov_len == size && memcmp(v.iov_base, expected, size) == 0);
      ++present;
    }
  }
  MDBX_stat stat;
  OK(mdbx_dbi_stat(txn, db, &stat, sizeof(stat)));
  CHECK(stat.ms_entries == present);
  free(expected);
}

static void run(unsigned pagesize) {
  char path[] = "/tmp/mdbx-prefault-integration-XXXXXX";
  CHECK(mkdtemp(path));
  MDBX_env *env;
  OK(mdbx_env_create(&env));
  OK(mdbx_env_set_geometry(env, 0, (intptr_t)pagesize * 2048, (intptr_t)pagesize * 4096, 0, 0, pagesize));
  OK(mdbx_env_set_option(env, MDBX_opt_prefault_write_enable, 1));
  OK(mdbx_env_open(env, path, MDBX_WRITEMAP | MDBX_NOSTICKYTHREADS | MDBX_NORDAHEAD, 0600));
  watched_env = env;
  source_writes = 0;
  for (unsigned generation = 0; generation < 8; ++generation)
    update(env, generation, pagesize, false);
  MDBX_txn *snapshot;
  OK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &snapshot));
  verify(snapshot, 7, pagesize);
  update(env, 8, pagesize, false);
  update(env, 9, pagesize, true);
  verify(snapshot, 7, pagesize);
  OK(mdbx_txn_abort(snapshot));
  MDBX_txn *latest;
  OK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &latest));
  verify(latest, 8, pagesize);
  OK(mdbx_txn_abort(latest));
  MDBX_envinfo info;
  OK(mdbx_env_info_ex(env, NULL, &info, sizeof(info)));
  CHECK(info.mi_pgop_stat.prefault > 0);
#if defined(__linux__) && !MDBX_MMAP_INCOHERENT_CPU_CACHE && !MDBX_MMAP_INCOHERENT_FILE_WRITE
  if (pagesize >= globals.sys_pagesize)
    CHECK(source_writes > 0);
#else
  CHECK(source_writes == 0);
#endif
  printf("public API %u-byte pages: %zu source copies, snapshot/abort verified\n", pagesize, source_writes);
  watched_env = NULL;
  OK(mdbx_env_close_ex(env, false));
  OK(mdbx_env_create(&env));
  OK(mdbx_env_open(env, path, MDBX_WRITEMAP | MDBX_NOSTICKYTHREADS | MDBX_NORDAHEAD, 0600));
  OK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &latest));
  verify(latest, 8, pagesize);
  OK(mdbx_txn_abort(latest));
  OK(mdbx_env_close_ex(env, false));
  OK(mdbx_env_delete(path, MDBX_ENV_JUST_DELETE));
}

int main(void) {
  run(4096);
  run(16384);
  run(65536);
  puts("prefault copy public API: update/delete, snapshot, abort and reopen passed");
  return 0;
}
