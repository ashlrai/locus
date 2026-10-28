import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';

export const repository = fileURLToPath(new URL('../../', import.meta.url));
const read = (root, path) => readFile(resolve(root, path), 'utf8');

export async function sources(root) {
  const cargo = await read(root, 'Cargo.toml');
  const section = cargo.match(/^\[workspace\.package\]\s*\n([\s\S]*?)(?=^\[|$(?![\s\S]))/m)?.[1];
  const version = section?.match(/^version\s*=\s*"(\d+\.\d+\.\d+(?:-[\w.-]+)?)"\s*$/m)?.[1];
  if (!version) throw new Error('Missing workspace.package.version');
  const cli = JSON.parse(await read(root, 'npm/package.json'));
  const mcp = JSON.parse(await read(root, 'npm-mcp/package.json'));
  const changelog = await read(root, 'CHANGELOG.md');
  const release = changelog.match(/^## \[(\d+\.\d+\.\d+(?:-[\w.-]+)?)\] (?:-|\u2014) (\d{4}-\d{2}-\d{2})\s*\n([\s\S]*?)(?=^## |$(?![\s\S]))/m);
  if (!release || [cli.version, mcp.version, release[1]].some(value => value !== version)) {
    throw new Error('Release sources disagree: Cargo, npm wrappers and latest released changelog must match');
  }
  const highlights = release[3].match(/^Highlights: (.+)$/m)?.[1];
  if (!highlights) throw new Error('Latest released changelog needs a Highlights: line');
  return { version, cli: cli.version, mcp: mcp.version, date: release[2], highlights };
}

export function render(template, values) {
  return template.replace(/\{\{(\w+)\}\}/g, (_, key) => {
    if (!(key in values)) throw new Error(`Unknown source: ${key}`);
    return values[key];
  });
}

export function replaceBlocks(content, blocks, templates, values) {
  const seen = new Set();
  const result = content.replace(/<!-- fresh:begin ([\w-]+) -->\n([\s\S]*?)<!-- fresh:end -->/g, (_, name) => {
    if (!blocks.includes(name) || seen.has(name) || !(name in templates)) {
      throw new Error(`Unknown or duplicate freshness block: ${name}`);
    }
    seen.add(name);
    return `<!-- fresh:begin ${name} -->\n${render(templates[name], values)}\n<!-- fresh:end -->`;
  });
  if (seen.size !== blocks.length) throw new Error('Missing or malformed freshness block');
  const outside = result.replace(/<!-- fresh:begin [\w-]+ -->\n[\s\S]*?<!-- fresh:end -->/g, '');
  if (/<!--\s*fresh:/.test(outside)) throw new Error('Malformed freshness marker');
  return result;
}

export async function run(root = repository, check = false) {
  const values = await sources(root);
  const { templates, files } = JSON.parse(await read(root, 'scripts/freshness/blocks.json'));
  // Validate everything before writing anything, including the source changelog.
  const changes = [];
  for (const [path, blocks] of Object.entries(files)) {
    const before = await read(root, path);
    const after = replaceBlocks(before, blocks, templates, values);
    if (before !== after) changes.push({ path, after });
  }
  if (!check) for (const { path, after } of changes) await writeFile(resolve(root, path), after);
  return changes.map(({ path }) => path);
}
