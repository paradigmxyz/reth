#!/usr/bin/env python3
"""Exercise the native ABI synchronously without a fuzzer or load-time execution threads."""
import ctypes
import json
import pathlib
import tempfile
import sys
import time


def assert_workers_joined(expected):
    # pthread_join may return before Linux removes the exited task from procfs.
    # Scoped Rust workers are joined synchronously; allow only bounded procfs cleanup.
    deadline = time.monotonic() + 1
    while True:
        tasks = list(pathlib.Path('/proc/self/task').iterdir())
        if len(tasks) == expected:
            return
        assert time.monotonic() < deadline, ('worker threads did not exit', [task.name for task in tasks])
        time.sleep(0.001)


def main():
    load_threads = len(list(pathlib.Path('/proc/self/task').iterdir()))
    library = ctypes.CDLL(str(pathlib.Path(sys.argv[1]).resolve()))
    assert len(list(pathlib.Path('/proc/self/task').iterdir())) == load_threads, 'library load spawned threads'
    pointer, size = ctypes.c_void_p, ctypes.c_size_t
    callback_type = ctypes.CFUNCTYPE(None, pointer, pointer, size)
    replies = []

    @callback_type
    def reply(context, data, length):
        replies.append(ctypes.string_at(data, length))

    create = library.rift_workload_create_v1
    create.argtypes = [pointer, size, callback_type, pointer]
    create.restype = pointer
    call = library.rift_workload_call_v1
    call.argtypes = [pointer, pointer, size, callback_type, pointer]
    call.restype = ctypes.c_int32
    destroy = library.rift_workload_destroy_v1
    destroy.argtypes = [pointer]
    destroy.restype = None

    def make(config):
        replies.clear()
        payload = json.dumps(config).encode()
        return create(payload, len(payload), reply, None)

    def invoke(handle, method, params, error=None):
        replies.clear()
        payload = json.dumps({'method': method, 'params': params}).encode()
        assert len(payload) <= 65_536, (method, len(payload))
        status = call(handle, payload, len(payload), reply, None)
        assert len(replies) == 1, (method, replies)
        text = replies[0].decode()
        if error is not None:
            assert status != 0 and text == error, (status, text)
            return
        assert status == 0, (method, text)
        return json.loads(text)

    assert not make({'args': {}, 'search': {'finalize_every': 2}})
    assert replies == [b'finalize_every must be 1']
    handle = make({'args': {}, 'search': {'finalize_every': 1}})
    assert handle and not replies
    try:
        args = {'args': {}}
        count = invoke(handle, 'seed-inputs', args)['count']
        assert count == 72
        inputs = [invoke(handle, 'seed-inputs', {**args, 'index': i})['input'] for i in range(count)]
        assert len(set(inputs)) == count
        # Prune mutations need explicit sites; leave the generated corpus unchanged.
        fixture = json.loads(inputs[0])
        fixture['rounds'][0].append({'kind': 'clone'})
        fixture['rounds'][-1].append({'kind': 'prune', 'cutoff': 100})
        fixture = json.dumps(fixture)
        donor = json.loads(inputs[1])
        donor['rounds'][0].append({'kind': 'clone'})
        donor['rounds'][-1].append({'kind': 'prune', 'cutoff': 100})
        donor = json.dumps(donor)
        invoke(handle, 'seed-inputs', {**args, 'index': count}, 'seed index is out of range')
        operators = invoke(handle, 'mutation-operators', args)
        mutated = []
        expected_operators = {
            'action.insert', 'action.remove', 'action.duplicate', 'action.crossover', 'action.swap',
            'update.insert', 'update.remove', 'update.duplicate', 'update.crossover',
            'key.update', 'key.prefix', 'value.update', 'prune.update',
        }
        assert {operator['name'] for operator in operators} == expected_operators
        for operator in operators:
            assert 'selector' not in operator, 'dynamic sites must not be bypassed'
            name = operator['name']
            sites = invoke(handle, 'mutation-sites', {**args, 'input': fixture, 'operator': name})
            assert sites and all(site['operator'] == name for site in sites), name
            mutation = {**args, 'input': fixture, 'seed': 99, 'operator': name,
                        'path': sites[0]['path'], 'donor': None, 'donor_path': []}
            if operator['donor']:
                donors = invoke(handle, 'mutation-sites', {**args, 'input': donor, 'operator': name})
                mutation.update(donor=donor, donor_path=donors[0]['path'])
            result = invoke(handle, 'mutate-input', mutation)
            assert isinstance(result, str) and result != fixture, name
            assert result == invoke(handle, 'mutate-input', mutation)
            original, changed = json.loads(fixture), json.loads(result)
            assert changed['version'] == 3 and changed['fixture_seed'] == original['fixture_seed']
            round_index = int(sites[0]['path'][1])
            assert all(changed['rounds'][i] == original['rounds'][i]
                       for i in range(16) if i != round_index), name
            mutated.append(result)
        invoke(handle, 'mutate-input', {**args, 'input': fixture, 'seed': 99,
               'operator': 'key.update', 'path': ['rounds', '99', '0', 'key']},
               'invalid mutation path')
        fresh = invoke(handle, 'fresh-input', {**args, 'seed': 123})
        assert fresh == invoke(handle, 'fresh-input', {**args, 'seed': 123})
        invoke(handle, 'finish', {'admitted': False, 'n': 0}, 'no execution is pending')
        invoke(handle, 'mutation-sites', {**args, 'input': fixture, 'operator': 'invalid'},
               'unknown mutation operator')
        # A malformed request is an ABI error, never a differential objective.
        replies.clear()
        bad = b'{'
        assert call(handle, bad, len(bad), reply, None) != 0 and len(replies) == 1
        with tempfile.TemporaryDirectory() as temporary:
            # The workload must never create the artifact directory or any execution file.
            before = set(pathlib.Path(temporary).iterdir())
            cases = (inputs if '--all-seeds' in sys.argv[2:] else [inputs[0], inputs[1], inputs[-1], fresh['input']]) + mutated
            objectives = 0
            layouts = set()
            edge_maps = set()
            native_slots = set()
            max_coverage_threads = 0
            for index, value in enumerate(cases):
                out = str(pathlib.Path(temporary) / str(index))
                result = invoke(handle, 'execute', {'input': value, 'seed': index, 'out': out})
                assert_workers_joined(load_threads)
                assert result['finalized']
                assert result['coverage'] == sorted(result['coverage'])
                assert all(0 <= slot < result['coverage_size'] and 1 <= count <= 255
                           for slot, count in result['coverage'])
                assert result['semantic_features'] == sorted(set(result['semantic_features']))
                assert result['coverage_id'].startswith('sparse-trie-sancov-v2-')
                assert 'uninstrumented' not in result['coverage_id']
                assert result['coverage_size'] > 100, 'missing real native edge layout'
                assert result['coverage'], 'no target edges were observed'
                assert 1 <= result['coverage_threads'] <= 2, result['coverage_threads']
                max_coverage_threads = max(max_coverage_threads, result['coverage_threads'])
                layouts.add((result['coverage_id'], result['coverage_size']))
                edge_maps.add(tuple(map(tuple, result['coverage'])))
                native_slots.update(slot for slot, _ in result['coverage'])
                if result['pending']:
                    assert result['coverage_id'] and result['coverage_size'] > 0
                    invoke(handle, 'seed-inputs', args, 'finish is required before another call')
                    finished = invoke(handle, 'finish', {'admitted': True, 'n': index})
                    assert finished == {**result, 'pending': False}
                else:
                    assert result['objective_signatures']
                    finished = result
                status = finished['verdict']['status']
                assert status in {'passed', 'difference'}, finished
                assert bool(finished['objective_signatures']) == (status == 'difference'), finished
                if status == 'passed':
                    report = finished['verdict']['report']
                    assert report['execution_modes'] == 2 and report['worker_threads'] == 2, report
                objectives += bool(finished['objective_signatures'])
                if index == 0:
                    replay_input, replay_result = value, result
            replay = invoke(handle, 'execute', {'input': replay_input, 'seed': 999, 'out': out})
            nondeterministic = {'duration_ns', 'coverage', 'coverage_threads'}
            assert {k: v for k, v in replay.items() if k not in nondeterministic} == {
                k: v for k, v in replay_result.items() if k not in nondeterministic}
            first_counts, replay_counts = dict(replay_result['coverage']), dict(replay['coverage'])
            replay_counter_changes = sum(first_counts.get(slot, 0) != replay_counts.get(slot, 0)
                                         for slot in first_counts.keys() | replay_counts.keys())
            if replay['pending']:
                invoke(handle, 'finish', {'admitted': False, 'n': len(cases)})
            assert max_coverage_threads == 2, 'instrumented target did not reach both workers'
            assert len(layouts) == 1, 'native slot layout changed between executions'
            assert len(edge_maps) > 1, 'native counters did not respond to different inputs'
            assert set(pathlib.Path(temporary).iterdir()) == before
            assert_workers_joined(load_threads)
        print(f'Native ABI smoke passed ({len(cases)} executions; {count} corpus entries; '
              f'{len(mutated)} structured mutations; {objectives} differential objectives; {len(native_slots)} native slots observed / '
              f'{next(iter(layouts))[1]} registered; deterministic semantic replay '
              f'({replay_counter_changes} native counts varied); target reached {max_coverage_threads} workers; '
              f'no files or load-time threads).')
    finally:
        destroy(handle)
    replies.clear()
    assert call(None, b'{}', 2, reply, None) != 0 and replies == [b'null workload handle']
    destroy(None)


if __name__ == '__main__':
    main()
