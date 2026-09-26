/* Standalone C workload for sanitizers and mdbx_chk. No Rust code is required.
 * Usage: parallel_audit DIRECTORY parallel|serial ROUNDS [overflow]
 * Each table has one worker. All workers join before serial child finalization.
 */
#include "mdbx.h"
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define TABLES 4
#define ROWS 256
#define CHECK(call) do { int rc_ = (call); if (rc_) { \
  fprintf(stderr, "%s:%d %s: %d %s\n", __FILE__, __LINE__, #call, rc_, mdbx_strerror(rc_)); \
  exit(1); } } while (0)

struct worker {
  MDBX_txn *txn;
  MDBX_dbi dbi;
  unsigned round;
  int overflow;
};

static void *write_table(void *arg) {
  struct worker *w = arg;
  unsigned char buffer[8192];
  memset(buffer, w->round + 1, sizeof(buffer));
  for (uint32_t i = 0; i < ROWS; ++i) {
    uint32_t key_bytes = __builtin_bswap32(i);
    MDBX_val key = { &key_bytes, sizeof(key_bytes) };
    MDBX_val data = { buffer, w->overflow && i % 4 == 0 ? sizeof(buffer) : 96 };
    CHECK(mdbx_put(w->txn, w->dbi, &key, &data, MDBX_UPSERT));
    if (w->round % 3 == 2 && i % 2 == 0)
      CHECK(mdbx_del(w->txn, w->dbi, &key, NULL));
  }
  return NULL;
}

static void verify(MDBX_txn *txn, MDBX_dbi *dbis, unsigned round) {
  for (unsigned t = 0; t < TABLES; ++t) {
    for (uint32_t i = 0; i < ROWS; ++i) {
      uint32_t key_bytes = __builtin_bswap32(i);
      MDBX_val key = { &key_bytes, sizeof(key_bytes) }, data;
      int rc = mdbx_get(txn, dbis[t], &key, &data);
      if (round % 3 == 2 && i % 2 == 0) {
        if (rc != MDBX_NOTFOUND) abort();
      } else {
        CHECK(rc);
        for (size_t j = 0; j < data.iov_len; ++j)
          if (((unsigned char *)data.iov_base)[j] != (unsigned char)(round + 1)) abort();
      }
    }
  }
}

int main(int argc, char **argv) {
  if (argc < 4) return 2;
  int parallel = strcmp(argv[2], "parallel") == 0;
  unsigned rounds = strtoul(argv[3], NULL, 10);
  MDBX_env *env;
  MDBX_txn *txn;
  MDBX_dbi dbis[TABLES];
  CHECK(mdbx_env_create(&env));
  CHECK(mdbx_env_set_maxdbs(env, 16));
  CHECK(mdbx_env_set_geometry(env, 0, -1, 512L << 20, -1, -1, 4096));
  CHECK(mdbx_env_open(env, argv[1], MDBX_WRITEMAP | MDBX_NOSTICKYTHREADS, 0600));
  CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  for (unsigned i = 0; i < TABLES; ++i) {
    char name[16];
    snprintf(name, sizeof(name), "t%u", i);
    CHECK(mdbx_dbi_open(txn, name, MDBX_CREATE, &dbis[i]));
  }
  CHECK(mdbx_txn_commit(txn));
  for (unsigned round = 0; round < rounds; ++round) {
    MDBX_txn *snapshot;
    CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &snapshot));
    CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
    MDBX_txn *children[TABLES];
    MDBX_subtxn_spec_t specs[TABLES];
    pthread_t threads[TABLES];
    struct worker workers[TABLES];
    for (unsigned i = 0; i < TABLES; ++i)
      specs[i] = (MDBX_subtxn_spec_t){dbis[i], 1};
    if (parallel) CHECK(mdbx_txn_create_subtxns(txn, specs, TABLES, children));
    for (unsigned i = 0; i < TABLES; ++i) {
      workers[i] = (struct worker){parallel ? children[i] : txn, dbis[i], round, argc > 4};
      if (parallel) CHECK(pthread_create(&threads[i], NULL, write_table, &workers[i]));
      else write_table(&workers[i]);
    }
    if (parallel) {
      for (unsigned i = 0; i < TABLES; ++i) CHECK(pthread_join(threads[i], NULL));
      for (unsigned i = 0; i < TABLES; ++i) CHECK(mdbx_subtx_commit(children[i]));
    }
    CHECK(mdbx_txn_commit(txn));
    if (round) verify(snapshot, dbis, round - 1);
    CHECK(mdbx_txn_abort(snapshot));
    CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &snapshot));
    verify(snapshot, dbis, round);
    CHECK(mdbx_txn_abort(snapshot));
    fprintf(stderr, "%s round %u verified\n", argv[2], round);
  }
  CHECK(mdbx_env_close(env));
  return 0;
}
