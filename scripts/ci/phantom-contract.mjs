#!/usr/bin/env node
// Release and Phantom contract check, run in CI after the release build.
//
// 1. The npm wrappers (locus-cli, @ashlrai/locus-mcp) are at the Cargo
//    workspace version and `npm pack` would ship their bins and install.js.
// 2. The release locus-mcp negotiates the MCP protocol versions Phantom's
//    companion smoke uses (scripts/companion-workflow-smoke.mjs in
//    ashlrai/phantom) and lists the tools Phantom requires.
//
// Never publishes and needs no credentials. Usage: node scripts/ci/phantom-contract.mjs
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const PROTOCOL_VERSIONS = ["2025-06-18", "2025-03-26"];
const REQUIRED_TOOLS = ["locus_whoami", "locus_status", "locus_request_pin"];
const mcpBin = process.env.MCP_BIN ?? join(root, "target", "release", "locus-mcp");

let failures = 0;
function report(name, ok, detail) {
  console.log(`${ok ? "ok  " : "FAIL"}  ${name}`);
  if (!ok) {
    failures += 1;
    if (detail) console.log(`      ${String(detail).trim().split("\n").join("\n      ")}`);
  }
}

const cargoVersion = readFileSync(join(root, "Cargo.toml"), "utf8").match(
  /\[workspace\.package\][^[]*?\nversion\s*=\s*"([^"]+)"/,
)?.[1];
report(`Cargo workspace version ${cargoVersion}`, Boolean(cargoVersion), "no [workspace.package] version");

for (const dir of ["npm", "npm-mcp"]) {
  const pkg = JSON.parse(readFileSync(join(root, dir, "package.json"), "utf8"));
  report(`${pkg.name} is at ${pkg.version}`, pkg.version === cargoVersion, `Cargo workspace is ${cargoVersion}`);
  const packed = spawnSync("npm", ["pack", "--dry-run", "--json", "--ignore-scripts"], { cwd: join(root, dir), encoding: "utf8" });
  if (packed.status !== 0) {
    report(`npm pack ${pkg.name}`, false, packed.stderr);
    continue;
  }
  const [info] = JSON.parse(packed.stdout.slice(packed.stdout.indexOf("[")));
  const files = new Set(info.files.map((f) => f.path));
  const missing = ["install.js", "README.md", "LICENSE", ...Object.values(pkg.bin)].filter((f) => !files.has(f));
  report(`${info.id} packs ${info.entryCount} files`, missing.length === 0, `missing ${missing.join(", ")}`);
}

const home = mkdtempSync(join(tmpdir(), "locus-phantom-contract-"));
try {
  for (const protocolVersion of PROTOCOL_VERSIONS) {
    const r = await handshake(mcpBin, { ...process.env, HOME: home, LOCUS_HOME: join(home, ".locus") }, protocolVersion);
    const absent = REQUIRED_TOOLS.filter((t) => !r.tools?.includes(t));
    report(
      `locus-mcp@${r.serverVersion} on ${protocolVersion}: ${r.tools?.length} tools`,
      !r.error && r.serverVersion === cargoVersion && absent.length === 0,
      r.error ?? `serverInfo.version ${r.serverVersion}, missing ${absent.join(", ")}`,
    );
  }
} finally {
  rmSync(home, { recursive: true, force: true });
}

if (failures > 0) {
  console.error(`\n${failures} contract check(s) failed`);
  process.exit(1);
}
console.log("\nall contract checks passed");

function handshake(bin, env, protocolVersion) {
  return new Promise((resolve) => {
    const child = spawn(bin, [], { env, stdio: ["pipe", "pipe", "pipe"] });
    const waiting = new Map();
    let buffer = "";
    let stderr = "";
    let nextId = 0;
    const done = (value) => {
      clearTimeout(timer);
      child.kill();
      resolve(value);
    };
    const timer = setTimeout(() => done({ error: `timed out\n${stderr.slice(-300)}` }), 20_000);
    child.once("error", (err) => done({ error: String(err) }));
    child.stderr.setEncoding("utf8").on("data", (c) => (stderr += c));
    child.stdout.setEncoding("utf8").on("data", (c) => {
      buffer += c;
      for (let i = buffer.indexOf("\n"); i >= 0; i = buffer.indexOf("\n")) {
        const line = buffer.slice(0, i).trim();
        buffer = buffer.slice(i + 1);
        if (!line) continue;
        try {
          const message = JSON.parse(line);
          waiting.get(message.id)?.(message);
        } catch {
          // stdout is protocol-only; a non-JSON line is reported by the missing reply
        }
      }
    });
    const request = (method, params = {}) =>
      new Promise((res) => {
        const id = ++nextId;
        waiting.set(id, res);
        child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
      });
    (async () => {
      const init = await request("initialize", { protocolVersion, capabilities: {}, clientInfo: { name: "locus-phantom-contract", version: "1" } });
      if (init.error) return done({ error: JSON.stringify(init.error) });
      child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" })}\n`);
      const list = await request("tools/list");
      done({ serverVersion: init.result?.serverInfo?.version, tools: (list.result?.tools ?? []).map((t) => t.name) });
    })();
  });
}
