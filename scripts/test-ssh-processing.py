#!/usr/bin/env python3
"""Explicit live fault test: metadata only; needs SSH and Python for fault injection.
The shipped engine itself does not require Python. Never starts reconstruction.
"""
import argparse, concurrent.futures, hashlib, json, shlex, subprocess, time, uuid
p = argparse.ArgumentParser()
for name in ('host', 'engine', 'folder', 'report'):
    p.add_argument('--' + name, required=True)
a = p.parse_args()
assert not a.host.startswith('-')
def ssh(command, body=None):
    r = subprocess.run(['ssh', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=5', a.host, command],
                       input=body, text=True, capture_output=True, timeout=45)
    if r.returncode:
        raise RuntimeError(r.stderr + r.stdout)
    return r.stdout
def rpc(v):
    out = json.loads(ssh(shlex.join([a.engine, 'agent']), json.dumps({'protocol': 1, **v})))
    if 'error' in out:
        raise RuntimeError(out['error'])
    return out
root = rpc({'action': 'inspect'})['root']
request = {'protocol': 1, 'action': 'submit', 'requestId': str(uuid.uuid4()),
           'command': 'tomo_session', 'args': {'folder': a.folder}, 'project': a.folder}
job = hashlib.sha256(request['requestId'].encode()).hexdigest()
# Reproduce a control-process crash between persisting and launching, in one new job.
seed = ('import pathlib,os\nos.umask(0o077)\np=pathlib.Path(' + repr(root) + ')/"jobs"/' + repr(job) +
        '\np.mkdir(parents=True)\n(p/"request.json").write_text(' + repr(json.dumps(request)) +
        ')\n(p/"state.json").write_text(\'{"state":"queued"}\')\n')
ssh('python3 -', seed)
with concurrent.futures.ThreadPoolExecutor(2) as pool:
    replies = list(pool.map(rpc, [request, request]))
assert all(r['job'] == job and r['reused'] for r in replies)
for _ in range(90):
    state = rpc({'action': 'status', 'job': job})
    if state['state'] in ('completed', 'failed', 'cancelled'):
        break
    time.sleep(.5)
assert state['state'] == 'completed', state
# A retry-spawned worker may be scheduled late, after the first worker finished.
ssh(shlex.join([a.engine, 'agent-worker', job]))
after = rpc({'action': 'status', 'job': job})
assert after == state, 'late duplicate worker changed or re-ran the finished task'
with open(a.report, 'w') as f:
    json.dump({'request': request, 'responses': replies, 'state': state,
               'prelaunchRecovery': True, 'lateWorkerDoesNotRerun': True}, f, indent=2)
print('PASS: persisted request recovers on concurrent retry; late worker does not rerun it')
