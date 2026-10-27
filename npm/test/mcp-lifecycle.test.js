const assert = require('node:assert/strict');
const { spawn, spawnSync } = require('node:child_process');
const fs = require('node:fs');
const { tmpdir } = require('node:os');
const { join, resolve } = require('node:path');
const test = require('node:test');

const wrapper = process.env.LOCUS_MCP_WRAPPER_UNDER_TEST || resolve(__dirname, '../../npm-mcp/bin/locus-mcp.js');
const marker = 'LOCUS_MCP_WRAPPER_ACTIVE';
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function fixture() {
  const home = fs.mkdtempSync(join(tmpdir(), 'locus-mcp-life-'));
  const cache = join(home, '.locus/bin');
  fs.mkdirSync(cache, { recursive: true });
  const pidFile = join(home, 'child.pid');
  const binary = join(cache, 'locus-mcp');
  // A shell launcher deliberately masks the node fixture as a native candidate.
  // The fixture ignores EOF, so the wrapper must enforce its own lifetime.
  const child = join(home, 'child.js');
  fs.writeFileSync(child, `require('fs').writeFileSync(${JSON.stringify(pidFile)}, String(process.pid)); process.stdin.resume(); setInterval(() => {}, 1000);`);
  fs.writeFileSync(binary, `#!/bin/sh\nexec '${process.execPath.replaceAll("'", "'\\''")}' '${child.replaceAll("'", "'\\''")}'\n`, { mode: 0o755 });
  return { home, cache, binary, pidFile, env: { HOME: home, USERPROFILE: home, PATH: '/usr/bin:/bin' } };
}

function alive(pid) {
  if (!Number.isInteger(pid) || pid <= 1) return false;
  try { process.kill(pid, 0); return true; } catch { return false; }
}
async function until(predicate, message, timeout = 5000) {
  const deadline = Date.now() + timeout;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error(message);
    await delay(25);
  }
}
async function ready(f) {
  await until(() => {
    try { return Number(fs.readFileSync(f.pidFile, 'utf8')) > 1; } catch { return false; }
  }, 'child did not start');
  return Number(fs.readFileSync(f.pidFile, 'utf8'));
}
function cleanup(f, ...processes) {
  for (const p of processes) { try { p.kill('SIGKILL'); } catch {} }
  try {
    const pid = Number(fs.readFileSync(f.pidFile, 'utf8'));
    if (Number.isInteger(pid) && pid > 1) process.kill(pid, 'SIGKILL');
  } catch {}
  fs.rmSync(f.home, { recursive: true, force: true });
}

test('locus-mcp: inherited recursion marker refuses before launching any candidate', () => {
  const f = fixture();
  // Baseline must finish safely; this fake candidate exits instead of leaking.
  fs.writeFileSync(f.binary, '#!/bin/sh\nexit 0\n', { mode: 0o755 });
  try {
    const result = spawnSync(process.execPath, [wrapper], {
      env: { ...f.env, [marker]: '1' }, encoding: 'utf8', timeout: 3000,
    });
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /recursive/i);
  } finally { cleanup(f); }
});

test('locus-mcp: twenty ended stdio sessions leave no wrapper or child processes', async () => {
  for (let i = 0; i < 20; i++) {
    const f = fixture();
    const proc = spawn(process.execPath, [wrapper], { env: f.env, stdio: ['pipe', 'pipe', 'pipe'] });
    let diagnostics = '';
    proc.stderr.on('data', chunk => { diagnostics += chunk; });
    try {
      const childPid = await ready(f).catch(error => {
        throw new Error(`session ${i}: ${error.message}; exit=${proc.exitCode}, signal=${proc.signalCode}; ${diagnostics}`);
      });
      proc.stdin.end();
      await until(() => proc.exitCode !== null, `session ${i}: wrapper survived stdin EOF`);
      await until(() => !alive(childPid), `session ${i}: child survived stdin EOF`);
      assert.equal(proc.exitCode, 0);
    } finally { cleanup(f, proc); }
  }
});

test('locus-mcp: losing its parent exits even with stdin still open', { skip: process.platform === 'win32' }, async () => {
  const f = fixture();
  const wrapperPidFile = join(f.home, 'wrapper.pid');
  const parentScript = join(f.home, 'parent.js');
  fs.writeFileSync(parentScript, `const c = require('child_process').spawn(process.execPath, [${JSON.stringify(wrapper)}], {stdio: ['inherit', 'ignore', 'ignore']}); require('fs').writeFileSync(${JSON.stringify(wrapperPidFile)}, String(c.pid)); setInterval(() => {}, 1000);`);
  const parent = spawn(process.execPath, [parentScript], { env: f.env, stdio: ['pipe', 'ignore', 'ignore'] });
  let wrapperPid;
  try {
    const childPid = await ready(f);
    wrapperPid = Number(fs.readFileSync(wrapperPidFile, 'utf8'));
    parent.kill('SIGKILL');
    await until(() => !alive(wrapperPid), 'wrapper survived parent death');
    await until(() => !alive(childPid), 'child survived wrapper parent death');
  } finally {
    if (wrapperPid) { try { process.kill(wrapperPid, 'SIGKILL'); } catch {} }
    cleanup(f, parent);
  }
});

for (const [label, args, extraEnv] of [
  ['--http', ['--http'], {}],
  ['-H', ['-H'], {}],
  ['--http=ADDR', ['--http=127.0.0.1:8742'], {}],
  ['LOCUS_MCP_HTTP', [], { LOCUS_MCP_HTTP: 'yes' }],
]) {
  test(`locus-mcp: HTTP mode (${label}) survives stdin EOF and forwards termination`, async () => {
    const f = fixture();
    const proc = spawn(process.execPath, [wrapper, ...args], { env: { ...f.env, ...extraEnv }, stdio: ['pipe', 'pipe', 'pipe'] });
    try {
      const childPid = await ready(f);
      proc.stdin.end();
      await delay(400);
      assert.equal(proc.exitCode, null);
      assert.ok(alive(childPid));
      proc.kill('SIGTERM');
      await until(() => proc.exitCode !== null || proc.signalCode !== null, 'wrapper ignored SIGTERM');
      await until(() => !alive(childPid), 'child survived SIGTERM');
    } finally { cleanup(f, proc); }
  });
}

test('locus-mcp: upstream marker rejects before resolving a candidate', () => {
  const f = fixture();
  try {
    const result = spawnSync(process.execPath, [wrapper], {
      env: { ...f.env, LOCUS_MCP_UPSTREAM_CHILD: '' }, encoding: 'utf8', timeout: 3000,
    });
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /recursive/i);
    assert.equal(fs.existsSync(f.pidFile), false);
  } finally { cleanup(f); }
});

test('locus-mcp: shell-disguised recursive candidate cannot launch a second child', async () => {
  const f = fixture();
  fs.writeFileSync(f.binary, `#!/bin/sh\nexec '${process.execPath.replaceAll("'", "'\\''")}' '${wrapper.replaceAll("'", "'\\''")}'\n`, { mode: 0o755 });
  const proc = spawn(process.execPath, [wrapper], { env: f.env, stdio: ['pipe', 'pipe', 'pipe'] });
  let stderr = '';
  proc.stderr.on('data', (chunk) => { stderr += chunk; });
  try {
    await until(() => proc.exitCode !== null, 'recursive shim did not fail promptly');
    assert.equal(proc.exitCode, 1);
    assert.match(stderr, /recursive/i);
  } finally { cleanup(f, proc); }
});

for (const location of ['cache', 'cargo', 'path']) {
  test(`locus-mcp: rejects Node shims from ${location} without executing them`, () => {
    const f = fixture();
    fs.unlinkSync(f.binary);
    const bin = join(f.home, 'bin');
    fs.mkdirSync(bin);
    fs.symlinkSync(process.execPath, join(bin, 'node'));
    fs.writeFileSync(join(bin, 'cargo'), '#!/bin/sh\nexit 1\n', { mode: 0o755 });
    const shim = location === 'cache' ? f.binary : location === 'cargo' ? join(f.home, '.cargo/bin/locus-mcp') : join(bin, 'locus-mcp');
    fs.mkdirSync(require('node:path').dirname(shim), { recursive: true });
    fs.writeFileSync(shim, `#!/usr/bin/env node\nrequire('fs').writeFileSync(${JSON.stringify(f.pidFile)}, 'executed');\n`, { mode: 0o755 });
    const preload = join(f.home, 'platform.js');
    fs.writeFileSync(preload, 'Object.defineProperty(process, "platform", {value: "linux"}); Object.defineProperty(process, "arch", {value: "arm64"});');
    try {
      const result = spawnSync(process.execPath, ['--require', preload, wrapper], {
        env: { ...f.env, PATH: `${bin}:/usr/bin:/bin` }, encoding: 'utf8', timeout: 5000,
      });
      assert.equal(result.status, 1, result.stderr);
      assert.match(result.stderr, /No prebuilt/);
      assert.equal(fs.existsSync(f.pidFile), false, `${location} Node shim was executed`);
    } finally { cleanup(f); }
  });
}

test('locus-mcp: one-shot version output survives closed stdin', () => {
  const f = fixture();
  fs.writeFileSync(f.binary, '#!/bin/sh\nsleep 0.1\nprintf "locus-mcp fixture\\n"\n', { mode: 0o755 });
  try {
    const result = spawnSync(process.execPath, [wrapper, '--version'], {
      env: f.env, encoding: 'utf8', timeout: 3000,
    });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, 'locus-mcp fixture\n');
  } finally { cleanup(f); }
});

test('locus-mcp: stdin EOF preserves the delayed final protocol response', async () => {
  const f = fixture();
  fs.writeFileSync(join(f.home, 'child.js'), `let input = ''; process.stdin.setEncoding('utf8'); process.stdin.on('data', chunk => { input += chunk; }); process.stdin.on('end', () => setTimeout(() => { process.stdout.write(input); }, 100));`);
  const proc = spawn(process.execPath, [wrapper], { env: f.env, stdio: ['pipe', 'pipe', 'pipe'] });
  let output = '';
  proc.stdout.on('data', chunk => { output += chunk; });
  try {
    const request = '{"jsonrpc":"2.0","id":1,"result":{}}\n';
    proc.stdin.end(request);
    await until(() => proc.exitCode !== null, 'wrapper did not drain final response');
    assert.equal(proc.exitCode, 0);
    assert.equal(output, request);
  } finally { cleanup(f, proc); }
});

for (const preparation of ['download', 'cargo', 'extractor']) {
  for (const disconnect of ['stdin EOF', 'parent death']) {
    test(`locus-mcp: ${disconnect} cancels stalled ${preparation} before server launch`, { skip: process.platform === 'win32' }, async () => {
      const f = fixture();
      fs.unlinkSync(f.binary);
      const preload = join(f.home, 'prepare.js');
      const started = join(f.home, 'preparation.started');
      const bin = join(f.home, 'bin');
      fs.mkdirSync(bin);
      if (preparation === 'extractor') {
        fs.writeFileSync(preload, `require('https').get = (url, callback) => { const request = new (require('events').EventEmitter)(); request.destroy = error => { setImmediate(() => request.emit('error', error)); return request; }; setImmediate(() => { const response = require('stream').Readable.from([Buffer.from('fake archive')]); response.statusCode = 200; callback(response); }); return request; };`);
        const extractor = join(f.home, 'extractor.js');
        fs.writeFileSync(extractor, `require('fs').writeFileSync(${JSON.stringify(f.pidFile)}, String(process.pid)); require('fs').writeFileSync(${JSON.stringify(started)}, 'started'); process.on('SIGTERM', () => {}); setInterval(() => {}, 1000);`);
        fs.writeFileSync(join(bin, 'tar'), `#!/bin/sh\nexec '${process.execPath.replaceAll("'", "'\\''")}' '${extractor.replaceAll("'", "'\\''")}'\n`, { mode: 0o755 });
      } else if (preparation === 'download') {
        // No network: model a request that never receives even response headers.
        fs.writeFileSync(preload, `require('https').get = () => { require('fs').writeFileSync(${JSON.stringify(started)}, 'started'); const request = new (require('events').EventEmitter)(); request.destroy = error => { setImmediate(() => request.emit('error', error)); return request; }; return request; };`);
      } else {
        fs.writeFileSync(preload, 'Object.defineProperty(process, "platform", {value: "linux"}); Object.defineProperty(process, "arch", {value: "arm64"});');
        const installer = join(f.home, 'installer.js');
        fs.writeFileSync(installer, `require('fs').writeFileSync(${JSON.stringify(f.pidFile)}, String(process.pid)); require('fs').writeFileSync(${JSON.stringify(started)}, 'started'); process.on('SIGTERM', () => {}); setInterval(() => {}, 1000);`);
        fs.writeFileSync(join(bin, 'cargo'), `#!/bin/sh\nexec '${process.execPath.replaceAll("'", "'\\''")}' '${installer.replaceAll("'", "'\\''")}'\n`, { mode: 0o755 });
      }
      const wrapperPidFile = join(f.home, 'wrapper.pid');
      const parentScript = join(f.home, 'parent.js');
      fs.writeFileSync(parentScript, `const child = require('child_process').spawn(process.execPath, ['--require', ${JSON.stringify(preload)}, ${JSON.stringify(wrapper)}], {stdio: ['inherit', 'ignore', 'ignore']}); require('fs').writeFileSync(${JSON.stringify(wrapperPidFile)}, String(child.pid)); child.on('exit', code => process.exit(code));`);
      const parent = spawn(process.execPath, [parentScript], { env: { ...f.env, PATH: `${bin}:/usr/bin:/bin` }, stdio: ['pipe', 'ignore', 'ignore'] });
      let wrapperPid;
      let installerPid;
      try {
        await until(() => fs.existsSync(started), `${preparation} did not start`);
        wrapperPid = Number(fs.readFileSync(wrapperPidFile, 'utf8'));
        if (preparation !== 'download') installerPid = await ready(f);
        if (disconnect === 'stdin EOF') parent.stdin.end();
        else parent.kill('SIGKILL');
        await until(() => !alive(wrapperPid), `${preparation} retained wrapper after ${disconnect}`);
        if (installerPid) await until(() => !alive(installerPid), 'preparation child survived cancelled initialization');
      } finally {
        if (wrapperPid) { try { process.kill(wrapperPid, 'SIGKILL'); } catch {} }
        cleanup(f, parent);
      }
    });
  }
}

test('locus-mcp: initialization preserves protocol bytes until async cargo finishes', async () => {
  const f = fixture();
  fs.unlinkSync(f.binary);
  const bin = join(f.home, 'bin');
  fs.mkdirSync(bin);
  const preload = join(f.home, 'prepare.js');
  fs.writeFileSync(preload, 'Object.defineProperty(process, "platform", {value: "linux"}); Object.defineProperty(process, "arch", {value: "arm64"});');
  const installer = join(f.home, 'installer.js');
  const installed = join(f.home, '.cargo/bin/locus-mcp');
  const readyFile = join(f.home, 'installer.started');
  const installedScript = `#!/bin/sh\nexec /bin/cat\n`;
  fs.writeFileSync(installer, `const fs = require('fs'); fs.writeFileSync(${JSON.stringify(readyFile)}, 'started'); setTimeout(() => { fs.mkdirSync(${JSON.stringify(require('node:path').dirname(installed))}, {recursive: true}); fs.writeFileSync(${JSON.stringify(installed)}, ${JSON.stringify(installedScript)}, {mode: 0o755}); }, 300);`);
  fs.writeFileSync(join(bin, 'cargo'), `#!/bin/sh\nexec '${process.execPath.replaceAll("'", "'\\''")}' '${installer.replaceAll("'", "'\\''")}'\n`, { mode: 0o755 });
  const proc = spawn(process.execPath, ['--require', preload, wrapper], { env: { ...f.env, PATH: `${bin}:/usr/bin:/bin` }, stdio: ['pipe', 'pipe', 'pipe'] });
  let output = '';
  let diagnostics = '';
  proc.stderr.on('data', chunk => { diagnostics += chunk; });
  proc.stdout.on('data', chunk => { output += chunk; });
  try {
    await until(() => fs.existsSync(readyFile), 'installer did not start');
    const request = '{"jsonrpc":"2.0","id":1,"method":"ping"}\n';
    proc.stdin.write(request);
    await until(() => output === request, 'protocol bytes lost during async install');
    proc.stdin.end();
    await until(() => proc.exitCode !== null, 'installed server did not exit');
    assert.equal(proc.exitCode, 0, diagnostics);
  } finally { cleanup(f, proc); }
});
