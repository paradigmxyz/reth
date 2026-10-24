/*
 * Copyright 2015 Vladimir Romanov
 * <https://www.linkedin.com/in/vladimirromanov>, Yota Lab.
 * SPDX-License-Identifier: AGPL-3.0
 *
 * You should have received a copy of the GNU Affero General Public License
 * along with this program.  If not, see <http://www.gnu.org/licenses/>. */

#include <sys/stat.h>
#include <sys/time.h>

#include "mdbx.h"
#include <assert.h>
#include <inttypes.h>
#include <limits.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define IP_PRINTF_ARG_HOST(addr)                                                                                       \
  (int)((addr) >> 24), (int)((addr) >> 16 & 0xff), (int)((addr) >> 8 & 0xff), (int)((addr) & 0xff)

char opt_db_path[PATH_MAX] = "./mdbx_bench2";
static MDBX_env *env;
#define REC_COUNT 10240000
int64_t ids[REC_COUNT * 10];
int32_t ids_count = 0;

int64_t mdbx_add_count = 0;
int64_t mdbx_del_count = 0;
uint64_t mdbx_add_time = 0;
uint64_t mdbx_del_time = 0;
int64_t obj_id = 0;
int64_t mdbx_data_size = 0;
int64_t mdbx_key_size = 0;

typedef struct {
  char session_id1[100];
  char session_id2[100];
  char ip[20];
  uint8_t fill[100];
} session_data_t;

typedef struct {
  int64_t obj_id;
  int8_t event_type;
} __attribute__((__packed__)) event_data_t;

static void add_id_to_pool(int64_t id) {
  ids[ids_count] = id;
  ids_count++;
}

static inline int64_t getClockUs(void) {
  struct timespec val;
#ifdef CYGWIN
  clock_gettime(CLOCK_REALTIME, &val);
#else
  clock_gettime(CLOCK_MONOTONIC, &val);
#endif
  return val.tv_sec * ((int64_t)1000000) + val.tv_nsec / 1000;
}

static int64_t get_id_from_pool() {
  if (ids_count == 0) {
    return -1;
  }
  int32_t index = rand() % ids_count;
  int64_t id = ids[index];
  ids[index] = ids[ids_count - 1];
  ids_count--;
  return id;
}

#define MDBX_CHECK(x)                                                                                                  \
  do {                                                                                                                 \
    const int rc = (x);                                                                                                \
    if (rc != MDBX_SUCCESS) {                                                                                          \
      printf("Error [%d] %s in %s at %s:%d\n", rc, mdbx_strerror(rc), #x, __FILE__, __LINE__);                         \
      exit(EXIT_FAILURE);                                                                                              \
    }                                                                                                                  \
  } while (0)

static void db_connect() {
  MDBX_dbi dbi_session;
  MDBX_dbi dbi_session_id;
  MDBX_dbi dbi_event;
  MDBX_dbi dbi_ip;

  MDBX_CHECK(mdbx_env_create(&env));
  MDBX_CHECK(mdbx_env_set_geometry(env, 0, 0, REC_COUNT * sizeof(session_data_t) * 10, -1, -1, -1));
  MDBX_CHECK(mdbx_env_set_maxdbs(env, 30));
  MDBX_CHECK(
      mdbx_env_open(env, opt_db_path, MDBX_CREATE | MDBX_WRITEMAP | MDBX_UTTERLY_NOSYNC | MDBX_LIFORECLAIM, 0664));
  MDBX_txn *txn;

  // transaction init
  MDBX_CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  // open database in read-write mode
  MDBX_CHECK(mdbx_dbi_open(txn, "session", MDBX_CREATE, &dbi_session));
  MDBX_CHECK(mdbx_dbi_open(txn, "session_id", MDBX_CREATE, &dbi_session_id));
  MDBX_CHECK(mdbx_dbi_open(txn, "event", MDBX_CREATE, &dbi_event));
  MDBX_CHECK(mdbx_dbi_open(txn, "ip", MDBX_CREATE, &dbi_ip));
  // transaction commit
  MDBX_CHECK(mdbx_txn_commit(txn));
  printf("Connection open\n");
}

static void create_record(uint64_t record_id) {
  MDBX_dbi dbi_session;
  MDBX_dbi dbi_session_id;
  MDBX_dbi dbi_event;
  MDBX_dbi dbi_ip;
  event_data_t event;
  MDBX_txn *txn;
  session_data_t data;
  // transaction init
  snprintf(data.session_id1, sizeof(data.session_id1), "prefix%02u_%02u.fill.fill.fill.fill.fill.fill;%" PRIu64,
           (unsigned)(record_id % 3) + 1, (unsigned)(record_id % 9) + 1, record_id);
  snprintf(data.session_id2, sizeof(data.session_id2), "dprefix%" PRIu64 ";%" PRIu64 ".fill.fill.;suffix", record_id,
           (record_id + UINT64_C(1442695040888963407)) % UINT64_C(6364136223846793005));
  snprintf(data.ip, sizeof(data.ip), "%d.%d.%d.%d", IP_PRINTF_ARG_HOST(record_id & 0xFFFFFFFF));
  event.obj_id = record_id;
  event.event_type = 1;

  MDBX_val _session_id1_rec = {data.session_id1, strlen(data.session_id1)};
  MDBX_val _session_id2_rec = {data.session_id2, strlen(data.session_id2)};
  MDBX_val _ip_rec = {data.ip, strlen(data.ip)};
  MDBX_val _obj_id_rec = {&record_id, sizeof(record_id)};
  MDBX_val _data_rec = {&data, offsetof(session_data_t, fill) + (rand() % sizeof(data.fill))};
  MDBX_val _event_rec = {&event, sizeof(event)};

  uint64_t start = getClockUs();
  MDBX_CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  MDBX_CHECK(mdbx_dbi_open(txn, "session", MDBX_CREATE, &dbi_session));
  MDBX_CHECK(mdbx_dbi_open(txn, "session_id", MDBX_CREATE, &dbi_session_id));
  MDBX_CHECK(mdbx_dbi_open(txn, "event", MDBX_CREATE, &dbi_event));
  MDBX_CHECK(mdbx_dbi_open(txn, "ip", MDBX_CREATE, &dbi_ip));
  MDBX_CHECK(mdbx_put(txn, dbi_session, &_obj_id_rec, &_data_rec, MDBX_NOOVERWRITE | MDBX_NODUPDATA));
  MDBX_CHECK(mdbx_put(txn, dbi_session_id, &_session_id1_rec, &_obj_id_rec, MDBX_NOOVERWRITE | MDBX_NODUPDATA));
  MDBX_CHECK(mdbx_put(txn, dbi_session_id, &_session_id2_rec, &_obj_id_rec, MDBX_NOOVERWRITE | MDBX_NODUPDATA));
  MDBX_CHECK(mdbx_put(txn, dbi_ip, &_ip_rec, &_obj_id_rec, 0));
  MDBX_CHECK(mdbx_put(txn, dbi_event, &_event_rec, &_obj_id_rec, 0));
  MDBX_CHECK(mdbx_txn_commit(txn));

  mdbx_data_size += (_data_rec.iov_len + _obj_id_rec.iov_len * 4);
  mdbx_key_size += (_obj_id_rec.iov_len + _session_id1_rec.iov_len + _session_id2_rec.iov_len + _ip_rec.iov_len +
                    _event_rec.iov_len);

  // transaction commit
  mdbx_add_count++;
  mdbx_add_time += (getClockUs() - start);
}

static void delete_record(int64_t record_id) {
  MDBX_dbi dbi_session;
  MDBX_dbi dbi_session_id;
  MDBX_dbi dbi_event;
  MDBX_dbi dbi_ip;
  event_data_t event;
  MDBX_txn *txn;

  // transaction init
  uint64_t start = getClockUs();
  MDBX_CHECK(mdbx_txn_begin(env, NULL, 0, &txn));
  // open database in read-write mode
  MDBX_CHECK(mdbx_dbi_open(txn, "session", MDBX_CREATE, &dbi_session));
  MDBX_CHECK(mdbx_dbi_open(txn, "session_id", MDBX_CREATE, &dbi_session_id));
  MDBX_CHECK(mdbx_dbi_open(txn, "event", MDBX_CREATE, &dbi_event));
  MDBX_CHECK(mdbx_dbi_open(txn, "ip", MDBX_CREATE, &dbi_ip));
  // put data
  MDBX_val _obj_id_rec = {&record_id, sizeof(record_id)};
  MDBX_val _data_rec;
  // get data
  MDBX_CHECK(mdbx_get(txn, dbi_session, &_obj_id_rec, &_data_rec));
  session_data_t *data = (session_data_t *)_data_rec.iov_base;

  MDBX_val _session_id1_rec = {data->session_id1, strlen(data->session_id1)};
  MDBX_val _session_id2_rec = {data->session_id2, strlen(data->session_id2)};
  MDBX_val _ip_rec = {data->ip, strlen(data->ip)};
  MDBX_CHECK(mdbx_del(txn, dbi_session_id, &_session_id1_rec, NULL));
  MDBX_CHECK(mdbx_del(txn, dbi_session_id, &_session_id2_rec, NULL));
  MDBX_CHECK(mdbx_del(txn, dbi_ip, &_ip_rec, NULL));
  event.obj_id = record_id;
  event.event_type = 1;
  MDBX_val _event_rec = {&event, sizeof(event)};
  MDBX_CHECK(mdbx_del(txn, dbi_event, &_event_rec, NULL));
  MDBX_CHECK(mdbx_del(txn, dbi_session, &_obj_id_rec, NULL));

  mdbx_data_size -= (_data_rec.iov_len + _obj_id_rec.iov_len * 4);
  mdbx_key_size -= (_obj_id_rec.iov_len + _session_id1_rec.iov_len + _session_id2_rec.iov_len + _ip_rec.iov_len +
                    _event_rec.iov_len);

  // transaction commit
  MDBX_CHECK(mdbx_txn_commit(txn));
  mdbx_del_count++;
  mdbx_del_time += (getClockUs() - start);
}

static void db_disconnect() {
  mdbx_env_close(env);
  printf("Connection closed\n");
}

static void get_db_stat(const char *db, int64_t *ms_branch_pages, int64_t *ms_leaf_pages) {
  MDBX_txn *txn;
  MDBX_stat stat;
  MDBX_dbi dbi;

  MDBX_CHECK(mdbx_txn_begin(env, NULL, MDBX_TXN_RDONLY, &txn));
  MDBX_CHECK(mdbx_dbi_open(txn, db, MDBX_DB_ACCEDE, &dbi));
  MDBX_CHECK(mdbx_dbi_stat(txn, dbi, &stat, sizeof(stat)));
  mdbx_txn_abort(txn);
  printf("%15s | %15" PRIu64 " | %5u | %10" PRIu64 " | %10" PRIu64 " | %11" PRIu64 " |\n", db, stat.ms_branch_pages,
         stat.ms_depth, stat.ms_entries, stat.ms_leaf_pages, stat.ms_overflow_pages);
  (*ms_branch_pages) += stat.ms_branch_pages;
  (*ms_leaf_pages) += stat.ms_leaf_pages;
}

static void periodic_stat(void) {
  int64_t ms_branch_pages = 0;
  int64_t ms_leaf_pages = 0;
  MDBX_stat mst;
  MDBX_envinfo mei;
  MDBX_CHECK(mdbx_env_stat_ex(env, NULL, &mst, sizeof(mst)));
  MDBX_CHECK(mdbx_env_info_ex(env, NULL, &mei, sizeof(mei)));
  printf("Environment Info\n");
  printf("  Pagesize: %u\n", mst.ms_psize);
  if (mei.mi_geo.lower != mei.mi_geo.upper) {
    printf("  Dynamic datafile: %" PRIu64 "..%" PRIu64 " bytes (+%" PRIu64 "/-%" PRIu64 "), %" PRIu64 "..%" PRIu64
           " pages (+%" PRIu64 "/-%" PRIu64 ")\n",
           mei.mi_geo.lower, mei.mi_geo.upper, mei.mi_geo.grow, mei.mi_geo.shrink, mei.mi_geo.lower / mst.ms_psize,
           mei.mi_geo.upper / mst.ms_psize, mei.mi_geo.grow / mst.ms_psize, mei.mi_geo.shrink / mst.ms_psize);
    printf("  Current datafile: %" PRIu64 " bytes, %" PRIu64 " pages\n", mei.mi_geo.current,
           mei.mi_geo.current / mst.ms_psize);
  } else {
    printf("  Fixed datafile: %" PRIu64 " bytes, %" PRIu64 " pages\n", mei.mi_geo.current,
           mei.mi_geo.current / mst.ms_psize);
  }
  printf("  Current mapsize: %" PRIu64 " bytes, %" PRIu64 " pages \n", mei.mi_mapsize, mei.mi_mapsize / mst.ms_psize);
  printf("  Number of pages used: %" PRIu64 "\n", mei.mi_last_pgno + 1);
  printf("  Last transaction ID: %" PRIu64 "\n", mei.mi_recent_txnid);
  printf("  Tail transaction ID: %" PRIu64 " (%" PRIi64 ")\n", mei.mi_latter_reader_txnid,
         mei.mi_latter_reader_txnid - mei.mi_recent_txnid);
  printf("  Max readers: %u\n", mei.mi_maxreaders);
  printf("  Number of readers used: %u\n", mei.mi_numreaders);

  printf("           Name | ms_branch_pages | depth |    entries | leaf_pages "
         "| overf_pages |\n");
  get_db_stat("session", &ms_branch_pages, &ms_leaf_pages);
  get_db_stat("session_id", &ms_branch_pages, &ms_leaf_pages);
  get_db_stat("event", &ms_branch_pages, &ms_leaf_pages);
  get_db_stat("ip", &ms_branch_pages, &ms_leaf_pages);
  printf("%15s | %15" PRIu64 " | %5s | %10s | %10" PRIu64 " | %11s |\n", "", ms_branch_pages, "", "", ms_leaf_pages,
         "");

  static int64_t prev_add_count;
  static int64_t prev_del_count;
  static uint64_t prev_add_time;
  static uint64_t prev_del_time;
  static int64_t t = -1;
  if (t > 0) {
    int64_t delta = (getClockUs() - t);
    printf("CPS: add %" PRIu64 ", delete %" PRIu64 ", items processed - %" PRIu64 "K data=%" PRIu64 "K key=%" PRIu64
           "K\n",
           (mdbx_add_count - prev_add_count) * 1000000 / delta, (mdbx_del_count - prev_del_count) * 1000000 / delta,
           obj_id / 1024, mdbx_data_size / 1024, mdbx_key_size / 1024);
    printf("usage data=%" PRIu64 "%%",
           ((mdbx_data_size + mdbx_key_size) * 100) / ((ms_leaf_pages + ms_branch_pages) * 4096));
    if (prev_add_time != mdbx_add_time) {
      printf(" Add : %" PRIu64 " c/s", (mdbx_add_count - prev_add_count) * 1000000 / (mdbx_add_time - prev_add_time));
    }
    if (prev_del_time != mdbx_del_time) {
      printf(" Del : %" PRIu64 " c/s", (mdbx_del_count - prev_del_count) * 1000000 / (mdbx_del_time - prev_del_time));
    }
    if (mdbx_add_time) {
      printf(" tAdd : %" PRIu64 " c/s", mdbx_add_count * 1000000 / mdbx_add_time);
    }
    if (mdbx_del_time) {
      printf(" tDel : %" PRIu64 " c/s", mdbx_del_count * 1000000 / mdbx_del_time);
    }
    puts("");
  }
  t = getClockUs();
  prev_add_count = mdbx_add_count;
  prev_del_count = mdbx_del_count;
  prev_add_time = mdbx_add_time;
  prev_del_time = mdbx_del_time;
}

#if 0 /* unused */
static void periodic_add_rec() {
  for (int i = 0; i < 10240; i++) {
    if (ids_count <= REC_COUNT) {
      int64_t id = obj_id++;
      create_record(id);
      add_id_to_pool(id);
    }
    if (ids_count > REC_COUNT) {
      int64_t id = get_id_from_pool();
      delete_record(id);
    }
  }
  periodic_stat();
}
#endif

int main(int argc, char **argv) {
  (void)argc;
  (void)argv;

  char filename[PATH_MAX];
  int i;

  strcpy(filename, opt_db_path);
  strcat(filename, MDBX_DATANAME);
  remove(filename);

  strcpy(filename, opt_db_path);
  strcat(filename, MDBX_LOCKNAME);
  remove(filename);

  puts("Open DB...");
  db_connect();
  puts("Create data...");
  int64_t t = getClockUs();
  for (i = 0; i < REC_COUNT; i++) {
    int64_t id = obj_id++;
    create_record(id);
    add_id_to_pool(id);
    if (i % 1000 == 0) {
      int64_t now = getClockUs();
      if ((now - t) > 1000000L) {
        periodic_stat();
        t = now;
      }
    }
  }
  periodic_stat();
  while (1) {
    int i;
    for (i = 0; i < 1000; i++) {
      int64_t id = obj_id++;
      create_record(id);
      add_id_to_pool(id);
      id = get_id_from_pool();
      delete_record(id);
    }
    for (i = 0; i < 50; i++) {
      int64_t id = obj_id++;
      create_record(id);
      add_id_to_pool(id);
    }
    int64_t id = obj_id++;
    create_record(id);
    add_id_to_pool(id);
    int64_t now = getClockUs();
    if ((now - t) > 10000000L) {
      periodic_stat();
      t = now;
    }
  }
  db_disconnect();
  return 0;
}
