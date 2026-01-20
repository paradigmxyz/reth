/** \file The example of using the libmdbx C API.
 * However, it is strongly recommended to use the modern C++ API, which requires less effort
 * and insures against many errors related to resource leaks.
 *
 * \note If you have already used Berkeley DB,
 * it will be useful to make a line-by-line comparison of this example and the sample-bdb.txt
 */

/*
 * Copyright 2015-2026 Leonid Yuriev <leo@yuriev.ru>.
 * Copyright 2017 Ilya Shipitsin <chipitsine@gmail.com>.
 * Copyright 2012-2015 Howard Chu, Symas Corp.
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted only as authorized by the OpenLDAP
 * Public License.
 *
 * A copy of this license is available in the file LICENSE in the
 * top-level directory of the distribution or, alternatively, at
 * <http://www.OpenLDAP.org/license.html>.
 */

#if (defined(__MINGW__) || defined(__MINGW32__) || defined(__MINGW64__)) && !defined(__USE_MINGW_ANSI_STDIO)
#define __USE_MINGW_ANSI_STDIO 1
#endif /* MinGW */

#include "mdbx.h"

#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char *argv[]) {
  (void)argc;
  (void)argv;

  int rc;
  MDBX_env *env = NULL;
  MDBX_dbi dbi = 0;
  MDBX_val key, data;
  MDBX_txn *txn = NULL;
  MDBX_cursor *cursor = NULL;
  char sval[32];

  printf("MDBX limits:\n");
#if UINTPTR_MAX > 0xffffFFFFul || ULONG_MAX > 0xffffFFFFul
  const double scale_factor = 1099511627776.0;
  const char *const scale_unit = "TiB";
#else
  const double scale_factor = 1073741824.0;
  const char *const scale_unit = "GiB";
#endif
  const size_t pagesize_min = mdbx_limits_pgsize_min();
  const size_t pagesize_max = mdbx_limits_pgsize_max();
  const size_t pagesize_default = mdbx_default_pagesize();

  printf("\tPage size: a power of 2, minimum %zu, maximum %zu bytes,"
         " default %zu bytes.\n",
         pagesize_min, pagesize_max, pagesize_default);
  printf("\tKey size: minimum %zu, maximum ≈¼ pagesize (%zu bytes for default"
         " %zuK pagesize, %zu bytes for %zuK pagesize).\n",
         (size_t)0, mdbx_limits_keysize_max(-1, MDBX_DB_DEFAULTS), pagesize_default / 1024,
         mdbx_limits_keysize_max(pagesize_max, MDBX_DB_DEFAULTS), pagesize_max / 1024);
  printf("\tValue size: minimum %zu, maximum %zu (0x%08zX) bytes for maps,"
         " ≈¼ pagesize for multimaps (%zu bytes for default %zuK pagesize,"
         " %zu bytes for %zuK pagesize).\n",
         (size_t)0, mdbx_limits_valsize_max(pagesize_min, MDBX_DB_DEFAULTS),
         mdbx_limits_valsize_max(pagesize_min, MDBX_DB_DEFAULTS), mdbx_limits_valsize_max(-1, MDBX_DUPSORT),
         pagesize_default / 1024, mdbx_limits_valsize_max(pagesize_max, MDBX_DUPSORT), pagesize_max / 1024);
  printf("\tWrite transaction size: up to %zu (0x%zX) pages (%f %s for default "
         "%zuK pagesize, %f %s for %zuK pagesize).\n",
         mdbx_limits_txnsize_max(pagesize_min) / pagesize_min, mdbx_limits_txnsize_max(pagesize_min) / pagesize_min,
         mdbx_limits_txnsize_max(-1) / scale_factor, scale_unit, pagesize_default / 1024,
         mdbx_limits_txnsize_max(pagesize_max) / scale_factor, scale_unit, pagesize_max / 1024);
  printf("\tDatabase size: up to %zu pages (%f %s for default %zuK "
         "pagesize, %f %s for %zuK pagesize).\n",
         mdbx_limits_dbsize_max(pagesize_min) / pagesize_min, mdbx_limits_dbsize_max(-1) / scale_factor, scale_unit,
         pagesize_default / 1024, mdbx_limits_dbsize_max(pagesize_max) / scale_factor, scale_unit, pagesize_max / 1024);
  printf("\tMaximum sub-databases: %u.\n", MDBX_MAX_DBI);
  printf("-----\n");

  rc = mdbx_env_create(&env);
  if (rc != MDBX_SUCCESS) {
    fprintf(stderr, "mdbx_env_create: (%d) %s\n", rc, mdbx_strerror(rc));
    goto bailout;
  }

  /* This deletion is not necessary, but it has been added here for full reproducibility of the conditions when running
   * this example again as a simplest test. */
  rc = mdbx_env_delete("./example-db", MDBX_ENV_JUST_DELETE);
  if (rc != /* file was successfully deleted */ MDBX_SUCCESS &&
      rc != /* nothing has been done because the file is missing */ MDBX_RESULT_TRUE) {
    fprintf(stderr, "mdbx_env_delete: (%d) %s\n", rc, mdbx_strerror(rc));
    goto bailout;
  }

  rc = mdbx_env_open(env, "./example-db", MDBX_NOSUBDIR | MDBX_LIFORECLAIM, 0664);
  if (rc != MDBX_SUCCESS) {
    fprintf(stderr, "mdbx_env_open: (%d) %s\n", rc, mdbx_strerror(rc));
    goto bailout;
  }

  rc = mdbx_txn_begin(env, NULL, 0, &txn);
  if (rc != MDBX_SUCCESS) {
    fprintf(stderr, "mdbx_txn_begin: (%d) %s\n", rc, mdbx_strerror(rc));
    goto bailout;
  }
  rc = mdbx_dbi_open(txn, NULL, 0, &dbi);
  if (rc != MDBX_SUCCESS) {
    fprintf(stderr, "mdbx_dbi_open: (%d) %s\n", rc, mdbx_strerror(rc));
    goto bailout;
  }

  key.iov_len = sizeof(int);
  key.iov_base = sval;
  data.iov_len = sizeof(sval);
  data.iov_base = sval;

  snprintf(sval, sizeof(sval), "%03x %d foo bar", 32, 3141592);
  rc = mdbx_put(txn, dbi, &key, &data, 0);
  if (rc != MDBX_SUCCESS) {
    fprintf(stderr, "mdbx_put: (%d) %s\n", rc, mdbx_strerror(rc));
    goto bailout;
  }
  rc = mdbx_txn_commit(txn);
  if (rc) {
    fprintf(stderr, "mdbx_txn_commit: (%d) %s\n", rc, mdbx_strerror(rc));
    goto bailout;
  }
  txn = NULL;

  rc = mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn);
  if (rc != MDBX_SUCCESS) {
    fprintf(stderr, "mdbx_txn_begin: (%d) %s\n", rc, mdbx_strerror(rc));
    goto bailout;
  }
  rc = mdbx_cursor_open(txn, dbi, &cursor);
  if (rc != MDBX_SUCCESS) {
    fprintf(stderr, "mdbx_cursor_open: (%d) %s\n", rc, mdbx_strerror(rc));
    goto bailout;
  }

  int found = 0;
  while ((rc = mdbx_cursor_get(cursor, &key, &data, MDBX_NEXT)) == 0) {
    printf("key: %p %.*s, data: %p %.*s\n", key.iov_base, (int)key.iov_len, (char *)key.iov_base, data.iov_base,
           (int)data.iov_len, (char *)data.iov_base);
    found += 1;
  }
  if (rc != MDBX_NOTFOUND || found == 0) {
    fprintf(stderr, "mdbx_cursor_get: (%d) %s\n", rc, mdbx_strerror(rc));
    goto bailout;
  } else {
    rc = MDBX_SUCCESS;
  }
bailout:
  if (cursor)
    mdbx_cursor_close(cursor);
  if (txn)
    mdbx_txn_abort(txn);
  if (dbi)
    mdbx_dbi_close(env, dbi);
  if (env)
    mdbx_env_close(env);
  return (rc != MDBX_SUCCESS) ? EXIT_FAILURE : EXIT_SUCCESS;
}
