#!/usr/bin/env python3
"""Exercise the native ABI synchronously without a fuzzer or execution threads."""
import ctypes
import json
import pathlib
import tempfile
import sys


def main():
    library = ctypes.CDLL(str(pathlib.Path(sys.argv[1]).resolve()))
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
        invoke(handle, 'seed-inputs', {**args, 'index': count}, 'seed index is out of range')
        operators = invoke(handle, 'mutation-operators', args)
        for operator in operators:
            name = operator['name']
            assert invoke(handle, 'mutation-sites', {**args, 'input': inputs[0], 'operator': name}) == [
                {'operator': name, 'path': ['seed']}]
            mutation = {**args, 'input': inputs[0], 'seed': 99, 'operator': name, 'path': ['seed']}
            result = invoke(handle, 'mutate-input', mutation)
            assert isinstance(result, str) and result != inputs[0]
            assert result == invoke(handle, 'mutate-input', mutation)
            assert isinstance(json.loads(result)['seed'], int)
        fresh = invoke(handle, 'fresh-input', {**args, 'seed': 123})
        assert fresh == invoke(handle, 'fresh-input', {**args, 'seed': 123})
        invoke(handle, 'finish', {'admitted': False, 'n': 0}, 'no execution is pending')
        invoke(handle, 'mutation-sites', {**args, 'input': inputs[0], 'operator': 'invalid'},
               'unknown mutation operator')
        # A malformed request is an ABI error, never a differential objective.
        replies.clear()
        bad = b'{'
        assert call(handle, bad, len(bad), reply, None) != 0 and len(replies) == 1
        with tempfile.TemporaryDirectory() as temporary:
            # The workload must never create the artifact directory or any execution file.
            before = set(pathlib.Path(temporary).iterdir())
            threads = len(list(pathlib.Path('/proc/self/task').iterdir()))
            cases = inputs if '--all-seeds' in sys.argv[2:] else [inputs[0], inputs[1], inputs[-1], fresh['input']]
            objectives = 0
            layouts = set()
            edge_maps = set()
            native_slots = set()
            for index, value in enumerate(cases):
                out = str(pathlib.Path(temporary) / str(index))
                result = invoke(handle, 'execute', {'input': value, 'seed': index, 'out': out})
                assert result['finalized']
                assert result['coverage'] == sorted(result['coverage'])
                assert all(0 <= slot < result['coverage_size'] and 1 <= count <= 255
                           for slot, count in result['coverage'])
                assert result['semantic_features'] == sorted(set(result['semantic_features']))
                assert result['coverage_id'].startswith('sparse-trie-sancov-v1-')
                assert 'uninstrumented' not in result['coverage_id']
                assert result['coverage_size'] > 100, 'missing real native edge layout'
                assert result['coverage'], 'no target edges were observed'
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
                objectives += bool(finished['objective_signatures'])
                # Other inputs and planning calls must not leak counters into replay. Ignore
                # duration only; host seed/out must not influence target execution or novelty.
                if index == 0:
                    replay_input, replay_result = value, result
            replay = invoke(handle, 'execute', {'input': replay_input, 'seed': 999, 'out': out})
            # Hash-table randomization can change native probe counts without changing trie
            # operations or oracle results. Do not mistake that noise for a semantic difference.
            nondeterministic = {'duration_ns', 'coverage'}
            assert {k: v for k, v in replay.items() if k not in nondeterministic} == {
                k: v for k, v in replay_result.items() if k not in nondeterministic}
            first_counts, replay_counts = dict(replay_result['coverage']), dict(replay['coverage'])
            replay_counter_changes = sum(first_counts.get(slot, 0) != replay_counts.get(slot, 0)
                                         for slot in first_counts.keys() | replay_counts.keys())
            if replay['pending']:
                invoke(handle, 'finish', {'admitted': False, 'n': len(cases)})
            assert len(layouts) == 1, 'native slot layout changed between executions'
            assert len(edge_maps) > 1, 'native counters did not respond to different inputs'
            assert set(pathlib.Path(temporary).iterdir()) == before
            assert len(list(pathlib.Path('/proc/self/task').iterdir())) == threads, 'execution spawned threads'
        print(f'Native ABI smoke passed ({len(cases)} executions; {count} corpus entries; '
              f'{objectives} differential objectives; {len(native_slots)} native slots observed / '
              f'{next(iter(layouts))[1]} registered; deterministic semantic replay '
              f'({replay_counter_changes} native counts varied); no files or threads).')
    finally:
        destroy(handle)
    replies.clear()
    assert call(None, b'{}', 2, reply, None) != 0 and replies == [b'null workload handle']
    destroy(None)


if __name__ == '__main__':
    main()
