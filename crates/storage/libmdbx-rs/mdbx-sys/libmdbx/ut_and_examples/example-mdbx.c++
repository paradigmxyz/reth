/// \copyright SPDX-License-Identifier: Apache-2.0
/// \author Леонид Юрьев aka Leonid Yuriev <leo@yuriev.ru> \date 2026
/// \file The example of using the libmdbx modern C++ API.

#include <iostream>
#include <mdbx.h++>

/* This is a minimal example now, which will be expanded soon. */

static bool doit(const mdbx::path &example_database_pathname) {
  using buffer = mdbx::buffer<mdbx::default_allocator, mdbx::default_capacity_policy>;
  mdbx::env::remove(example_database_pathname);
  mdbx::env_managed env(example_database_pathname, mdbx::env_managed::create_parameters(),
                        mdbx::env::operate_parameters(11));

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
    return doit("example_database") ? EXIT_SUCCESS : EXIT_FAILURE;
  } catch (const std::exception &ex) {
    std::cerr << "Exception: " << ex.what() << "\n";
    return EXIT_FAILURE;
  }
}
