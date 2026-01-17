/// \copyright SPDX-License-Identifier: Apache-2.0
/// \author Леонид Юрьев aka Leonid Yuriev <leo@yuriev.ru> \date 2026
/// \file The example of using the libmdbx modern C++ API.

#include <iostream>
#include <mdbx.h++>

/* This is a minimal example now, which will be expanded soon. */

static void тысяча(const mdbx::path &database_pathname, const mdbx::env::mode mode, mdbx::env::durability durability) {
  mdbx::env::remove(database_pathname);
  std::cout << "INSERTIONx1000(" << mode << ", " << durability << ")" << std::endl;

  mdbx::env_managed env(database_pathname, mdbx::env_managed::create_parameters(),
                        mdbx::env::operate_parameters(3, 0, mode, durability));
  for (int i = 0; i < 1000; ++i) {
    auto txn = env.start_write();
    auto map = txn.create_map("table-data", mdbx::key_mode::usual, mdbx::value_mode::single);

    auto k = std::to_string(i);
    auto v = mdbx::to_base58(mdbx::slice::wrap(i)).as_string();
    txn.insert(map, mdbx::slice(k), mdbx::slice(v));
    txn.commit();
  }

  auto info = env.get_info();

  // std::cout << "  pgop.newly: " << info.mi_pgop_stat.newly << "\n";
  // std::cout << "  pgop.cow: " << info.mi_pgop_stat.cow << "\n";
  // std::cout << "  pgop.clone: " << info.mi_pgop_stat.clone << "\n";
  // std::cout << "  pgop.split: " << info.mi_pgop_stat.split << "\n";
  // std::cout << "  pgop.merge: " << info.mi_pgop_stat.merge << "\n  --\n";

  // std::cout << "  pgop.spill: " << info.mi_pgop_stat.spill << "\n";
  // std::cout << "  pgop.unspill: " << info.mi_pgop_stat.unspill << "\n";
  // std::cout << "  pgop.mincore: " << info.mi_pgop_stat.mincore << "\n  --\n";

  std::cout << "  pgop.prefault: " << info.mi_pgop_stat.prefault << "\n";
  std::cout << "  pgop.msync: " << info.mi_pgop_stat.msync << "\n";
  std::cout << "  pgop.fsync: " << info.mi_pgop_stat.fsync << "\n";
  std::cout << "  pgop.wops: " << info.mi_pgop_stat.wops << "\n=====\n" << std::endl;
}

static bool doit(const mdbx::path &database_pathname) {
  using buffer = mdbx::buffer<mdbx::default_allocator, mdbx::default_capacity_policy>;
  mdbx::env::remove(database_pathname);
  mdbx::env_managed env(database_pathname, mdbx::env_managed::create_parameters(), mdbx::env::operate_parameters(11));

  auto txn = env.start_write();
  auto map = txn.create_map("table-ordinals", mdbx::key_mode::ordinal, mdbx::value_mode::single);
  txn.insert(map, buffer::key_from_u64(42), "a");
  txn.insert(map, buffer::key_from_double(0.1), mdbx::slice("b"));
  txn.insert(map, buffer::key_from_jsonInteger(1), buffer("c"));
  txn.insert(map, mdbx::slice::wrap(uint64_t(0xaBad1dea)), buffer::base58("aBad1dea"));
  txn.commit_embark_read();

  auto cursor = txn.open_cursor(map);
  cursor.to_first();
  while (!cursor.eof()) {
    std::cout << cursor.current() << std::endl;
    cursor.to_next(false);
  }

  return true;
}

int main(int, const char *[]) {
  try {
    const mdbx::path bench_database =
#if !(defined(_WIN32) || defined(_WIN64))
        "/tmp/"
#endif /* !Windows */
        "bench_example_database.mdbx";
    тысяча(bench_database, mdbx::env::mode::write_file_io, mdbx::env::durability::robust_synchronous);
    тысяча(bench_database, mdbx::env::mode::write_file_io, mdbx::env::durability::half_synchronous_weak_last);
    тысяча(bench_database, mdbx::env::mode::write_file_io, mdbx::env::durability::lazy_weak_tail);
    тысяча(bench_database, mdbx::env::mode::write_file_io, mdbx::env::durability::whole_fragile);
    тысяча(bench_database, mdbx::env::mode::write_mapped_io, mdbx::env::durability::robust_synchronous);
    тысяча(bench_database, mdbx::env::mode::write_mapped_io, mdbx::env::durability::half_synchronous_weak_last);
    тысяча(bench_database, mdbx::env::mode::write_mapped_io, mdbx::env::durability::lazy_weak_tail);
    тысяча(bench_database, mdbx::env::mode::write_mapped_io, mdbx::env::durability::whole_fragile);
    return doit("example_database") ? EXIT_SUCCESS : EXIT_FAILURE;
  } catch (const std::exception &ex) {
    std::cerr << "Exception: " << ex.what() << "\n";
    return EXIT_FAILURE;
  }
}
