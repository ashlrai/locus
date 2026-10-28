#!/usr/bin/env python3
"""Cross-binary acceptance: run after `cargo build --workspace` from the repo root.

Uses only a temporary HOME/LOCUS_HOME and credential-free fixture workers.
Cargo's per-package integration tests cannot locate the other package's binary
through CARGO_BIN_EXE, so this explicitly verifies the installed CLI boundary.
"""
import json, os, pathlib, subprocess, tempfile

root = pathlib.Path.cwd()
locus = root / 'target/debug/locus'
mcp = root / 'target/debug/locus-mcp'
with tempfile.TemporaryDirectory(prefix='locus-router-cli-') as temporary:
    base = pathlib.Path(temporary)
    env = {k:v for k,v in os.environ.items() if not k.startswith(('LOCUS_', 'GH_', 'GITHUB_', 'SUPABASE_', 'VERCEL_', 'AWS_'))}
    env.update(HOME=str(base), LOCUS_HOME=str(base/'store'), LOCUS_CONTROL_CAPABILITY='ab'*32, LOCUS_MCP_AUTO_PIN='0')
    def cli(args, extra=None):
        return subprocess.run([str(locus), *args], env={**env, **(extra or {})}, cwd=base,
                              stdin=subprocess.DEVNULL, capture_output=True, timeout=30)
    result = cli(['init', '--no-persist-capability'])
    assert result.returncode == 0, 'isolated init failed'
    worker = base / 'worker.py'
    worker.write_text('''import json, os, sys
for line in sys.stdin:
    message=json.loads(line)
    if 'id' not in message: continue
    method=message['method']
    if method == 'initialize': result={'protocolVersion':'2024-11-05','capabilities':{'tools':{}},'serverInfo':{'name':'router-fixture','version':'1'}}
    elif method == 'tools/list': result={'tools':[{'name':'ping','inputSchema':{'type':'object'}}]}
    else: result={'content':[{'type':'text','text':os.environ['LOCUS_WORKER_PROVIDER']}],'isError':False}
    print(json.dumps({'jsonrpc':'2.0','id':message['id'],'result':result}), flush=True)
''')
    binding = '''[binding]
id = "bnd_router_cli"
alias = "router"
tenant = "router-fixture"
[binding.policy]
default = "allow"
'''
    for provider in ['github', 'supabase', 'vercel']:
        binding += f'''\n[[binding.providers]]
provider = "{provider}"
account = "fixture"
credential_ref = "phm:ROUTER_FIXTURE"
[binding.providers.upstream]
command = "python3"
args = ["-u", {json.dumps(str(worker))}]
resolve_secrets = false
'''
    (base/'store/bindings/router.toml').write_text(binding)
    result = cli(['enter', 'router'])
    assert result.returncode == 0, 'isolated enter failed'
    requests = [
        ('initialize', {'protocolVersion':'2024-11-05','capabilities':{},'clientInfo':{'name':'cli-smoke','version':'1'}}),
        ('tools/list', {}),
        ('tools/call', {'name':'github.ping','arguments':{}}),
        ('tools/call', {'name':'supabase.ping','arguments':{}}),
        ('tools/call', {'name':'vercel.scope','arguments':{}}),
        ('tools/list', {}),
    ]
    payload = ''.join(json.dumps({'jsonrpc':'2.0','id':i,'method':method,'params':params})+'\n' for i,(method,params) in enumerate(requests,1))
    result = subprocess.run([str(locus), 'mcp', '--route', 'github', '--route', 'supabase', '--mcp-bin', str(mcp)],
                            env=env, cwd=base, input=payload.encode(), capture_output=True, timeout=45)
    assert result.returncode == 0, 'CLI router failed'
    assert b'supervised as router' in result.stderr, 'route must imply supervised'
    replies = [json.loads(line) for line in result.stdout.splitlines()]
    assert len(replies) == 6
    names = [tool['name'] for tool in replies[1]['result']['tools']]
    assert 'github.scope' in names and 'supabase.scope' in names
    assert not any(name.startswith('vercel.') for name in names)
    for reply, provider in zip(replies[2:4], ['github','supabase']):
        assert not reply['result']['isError'], 'provider call failed'
        assert provider in reply['result']['content'][0]['text']
    assert replies[4]['result']['isError']
    assert 'route_not_enabled' in replies[4]['result']['content'][0]['text']
    names = [tool['name'] for tool in replies[5]['result']['tools']]
    assert 'github.ping' in names and 'supabase.ping' in names
    marker = base / 'should-not-launch'
    stub = base / 'stub-mcp'
    stub.write_text('#!/bin/sh\ntouch '+str(marker)+'\n')
    stub.chmod(0o700)
    for variable in ['LOCUS_MCP_UPSTREAM_CHILD', 'LOCUS_WORKER_PROVIDER']:
        for value in ['', '1']:
            result = cli(['mcp', '--route', 'github', '--mcp-bin', str(stub)], {variable:value})
            assert result.returncode != 0 and b'recursive' in result.stderr
            assert not result.stdout and not marker.exists()
    print('PASS: actual CLI forwards two routes, implicitly supervises, routes two workers, filters excluded provider, and refuses both recursion markers (4 cases) before child launch')
