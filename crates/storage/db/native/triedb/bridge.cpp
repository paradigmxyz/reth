// SPDX-License-Identifier: GPL-3.0-or-later
// Adapter to Category Labs' GPL-3.0-or-later TrieDB library.
#include <category/core/log.hpp>
#include <category/mpt/compute.hpp>
#include <category/mpt/db.hpp>
#include <category/mpt/db_error.hpp>
#include <category/mpt/ondisk_db_config.hpp>
#include <category/mpt/state_machine.hpp>
#include <category/mpt/update.hpp>
#include <category/mpt/util.hpp>

#include <algorithm>
#include <array>
#include <cstring>
#include <fcntl.h>
#include <memory>
#include <mutex>
#include <stdexcept>
#include <string>
#include <sys/file.h>
#include <unistd.h>
#include <vector>

namespace {
using namespace monad::mpt;
constexpr size_t key_size = 100;
using Key = std::array<unsigned char, key_size>;
thread_local std::string last_error;

// Reth retains its Ethereum merklization. This trie is the physical index for
// account and storage records, so it need not calculate a second commitment.
class RethStateMachine final : public monad::mpt::StateMachine {
    size_t depth_{};
public:
    std::unique_ptr<monad::mpt::StateMachine> clone() const override {
        return std::make_unique<RethStateMachine>(*this);
    }
    void down(unsigned char) override { ++depth_; }
    void up(size_t count) override { depth_ -= count; }
    Compute &get_compute() const override { static EmptyCompute compute; return compute; }
    bool cache() const override { return depth_ < 8; }
    bool compact() const override { return true; }
    bool is_variable_length() const override { return false; }
};

struct Handle {
    std::mutex writer_mutex;
    std::unique_ptr<AsyncIOContext> read_context;
    std::unique_ptr<Db> writer;
    std::unique_ptr<RODb> reader;
    int sync_fd{-1};
    ~Handle() { reader.reset(); writer.reset(); if (sync_fd >= 0) ::close(sync_fd); }
};

struct Record { Key key; std::vector<unsigned char> value; };

struct Seek final : TraverseMachine {
    Key bound;
    bool reverse;
    bool found{false};
    Record result{};
    std::vector<unsigned char> path;
    std::vector<size_t> sizes;

    Seek(Key bound_, bool reverse_) : bound(bound_), reverse(reverse_) {}
    bool admissible(std::vector<unsigned char> const &prefix) const {
        for (size_t i = 0; i < prefix.size(); ++i) {
            auto nibble = (bound[i / 2] >> (i % 2 == 0 ? 4 : 0)) & 15;
            if (prefix[i] != nibble) return reverse ? prefix[i] < nibble : prefix[i] > nibble;
        }
        return true;
    }
    bool down(unsigned char branch, Node const &node) override {
        size_t old_size = path.size();
        // TrieDB's synthetic root carries metadata, not a user record.
        if (branch == INVALID_BRANCH) { sizes.push_back(old_size); return true; }
        path.push_back(branch);
        auto suffix = node.path_nibble_view();
        for (unsigned i = 0; i < suffix.nibble_size(); ++i) path.push_back(suffix.get(i));
        if (path.size() > key_size * 2) throw std::runtime_error("unexpected TrieDB key length");
        if (found || !admissible(path)) { path.resize(old_size); return false; }
        if (node.has_value()) {
            if (path.size() != key_size * 2) throw std::runtime_error("unexpected TrieDB leaf length: " + std::to_string(path.size()));
            for (size_t i = 0; i < key_size; ++i) result.key[i] = (path[2*i] << 4) | path[2*i+1];
            auto value = node.value();
            result.value.assign(value.begin(), value.end());
            found = true;
            path.resize(old_size);
            return false;
        }
        sizes.push_back(old_size);
        return true;
    }
    void up(unsigned char, Node const &) override { path.resize(sizes.back()); sizes.pop_back(); }
    bool should_visit(Node const &, unsigned char branch) override {
        if (found) return false;
        path.push_back(branch);
        bool visit = admissible(path);
        path.pop_back();
        return visit;
    }
    std::unique_ptr<TraverseMachine> clone() const override { return std::make_unique<Seek>(*this); }
};

template<class F> int guarded(F &&f) noexcept {
    try { return f(); }
    catch (std::exception const &e) { last_error = e.what(); return -1; }
    catch (...) { last_error = "unknown native TrieDB exception"; return -1; }
}
}

extern "C" {
struct RethTrieUpdate { unsigned char const *key; unsigned char const *value; size_t len; bool deleted; };
uint32_t reth_triedb_abi_version() noexcept { return 1; }
char const *reth_triedb_error() { return last_error.c_str(); }

int reth_triedb_open(char const *path, bool create, bool readonly, void **out) noexcept {
    return guarded([&] {
        static std::once_flag logging;
        std::call_once(logging, [] { monad::init_root_logger(quill::LogLevel::Warning); });
        auto h = std::make_unique<Handle>();
        OnDiskDbConfig config;
        config.dbname_path = path;
        config.append = !create;
        config.compaction = true;
        config.sq_thread_cpu = std::nullopt;
        config.file_size_db = 16;
        config.fixed_history_length = 1000000;
        ReadOnlyOnDiskDbConfig ro_config;
        ro_config.dbname_path = path;
        if (readonly) {
            if (create) throw std::runtime_error("cannot initialize a read-only TrieDB");
            h->read_context = std::make_unique<AsyncIOContext>(ro_config);
            h->writer = std::make_unique<Db>(*h->read_context);
        } else {
            // Acquire the writer lease before any potentially destructive initialization.
            bool const exists = std::filesystem::exists(path);
            h->sync_fd = ::open(path, O_RDWR | O_CLOEXEC | (create ? O_CREAT : 0), 0600);
            if (h->sync_fd < 0) throw std::runtime_error("cannot open TrieDB for durable commits");
            if (::flock(h->sync_fd, LOCK_EX | LOCK_NB) != 0)
                throw std::runtime_error("TrieDB device already has a writer");
            if (!exists && ::ftruncate(h->sync_fd, config.file_size_db * 1024 * 1024 * 1024 + 24576) != 0)
                throw std::runtime_error("cannot size TrieDB file");
            h->writer = std::make_unique<Db>(std::make_unique<RethStateMachine>(), config);
        }
        h->reader = std::make_unique<RODb>(ro_config);
        *out = h.release();
        return 0;
    });
}
void reth_triedb_close(void *handle) noexcept { delete static_cast<Handle *>(handle); }
void reth_triedb_record_free(void *record) noexcept { delete static_cast<Record *>(record); }
unsigned char const *reth_triedb_record_key(void *record) noexcept { return static_cast<Record *>(record)->key.data(); }
unsigned char const *reth_triedb_record_value(void *record, size_t *len) noexcept {
    auto &value = static_cast<Record *>(record)->value;
    *len = value.size();
    return value.data();
}
int reth_triedb_get(void *handle, uint64_t version, unsigned char const *key, void **out) noexcept {
    return guarded([&] {
        auto &h = *static_cast<Handle *>(handle);
        auto result = h.reader->find(NibblesView(0, 2 * key_size, key), version);
        if (!result) {
            if (result.error().value() == static_cast<int>(DbError::key_not_found)) return 0;
            throw std::runtime_error("TrieDB snapshot missing or read failed");
        }
        auto record = std::make_unique<Record>();
        std::copy_n(key, key_size, record->key.begin());
        auto value = result.value().node->value();
        record->value.assign(value.begin(), value.end());
        *out = record.release();
        return 1;
    });
}
int reth_triedb_seek(void *handle, uint64_t version, unsigned char const *key, bool reverse, void **out) noexcept {
    return guarded([&] {
        auto &h = *static_cast<Handle *>(handle);
        std::lock_guard lock(h.writer_mutex);
        auto root = h.writer->load_root_for_version(version);
        if (!root) throw std::runtime_error("TrieDB snapshot missing during seek");
        Key bound;
        std::copy_n(key, key_size, bound.begin());
        Seek machine(bound, reverse);
        auto children = [reverse](uint16_t mask) {
            std::vector<std::pair<unsigned, unsigned char>> result;
            for (auto const &[index, branch] : NodeChildrenRange(mask)) result.emplace_back(index, branch);
            if (reverse) std::reverse(result.begin(), result.end());
            return result;
        };
        if (!h.writer->traverse_blocking(NodeCursor(root), machine, version, children))
            throw std::runtime_error("TrieDB seek failed");
        if (!machine.found) return 0;
        *out = new Record(std::move(machine.result));
        return 1;
    });
}
int reth_triedb_commit(void *handle, uint64_t base, RethTrieUpdate const *updates, size_t count, uint64_t *version) noexcept {
    return guarded([&] {
        auto &h = *static_cast<Handle *>(handle);
        if (h.sync_fd < 0) throw std::runtime_error("commit on read-only TrieDB");
        std::lock_guard lock(h.writer_mutex);
        auto latest = h.writer->get_latest_version();
        *version = latest == UINT64_MAX ? 0 : latest + 1;
        auto root = base == UINT64_MAX ? Node::SharedPtr{} : h.writer->load_root_for_version(base);
        if (base != UINT64_MAX && !root) throw std::runtime_error("TrieDB base snapshot missing");
        std::vector<Update> native_updates(count);
        UpdateList list;
        for (size_t i = 0; i < count; ++i) {
            auto &update = native_updates[i];
            update.key = NibblesView(0, 2 * key_size, updates[i].key);
            update.version = static_cast<int64_t>(*version);
            if (!updates[i].deleted) update.value = monad::byte_string_view(updates[i].value, updates[i].len);
            list.push_front(update);
        }
        h.writer->upsert(std::move(root), std::move(list), *version);
        if (::fdatasync(h.sync_fd) != 0) throw std::runtime_error("TrieDB fdatasync failed");
        return 0;
    });
}
}
