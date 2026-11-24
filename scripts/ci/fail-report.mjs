// First cargo or nextest failure, as a file:line annotation and a short excerpt.
// The full compiler or test log stays off the step output.

export function stripAnsi(text) {
  return text.replace(/\u001b\[[0-9;]*m/g, "");
}

function excerptAround(text, index) {
  const start = text.lastIndexOf("\n", Math.max(0, index - 1));
  const from = Math.max(0, text.lastIndexOf("\n---- ", start) === -1
    ? text.lastIndexOf("\n", Math.max(0, index - 500))
    : text.lastIndexOf("\n---- ", start));
  const slice = text.slice(Math.max(0, from), Math.min(text.length, index + 700));
  const lines = slice.split("\n").filter((line) => line.trim() !== "");
  return lines.slice(0, 40).join("\n");
}

function cleanPath(file) {
  return file.replaceAll("\\", "/").replace(/^\.\//, "");
}

function errorArrow(text) {
  const pattern = /^\s*--> (?<file>[^\s:]+):(?<line>\d+):\d+/gm;
  let match;
  while ((match = pattern.exec(text))) {
    const before = text.slice(Math.max(0, match.index - 500), match.index);
    const warningAt = before.lastIndexOf("warning:");
    const errorAt = Math.max(before.lastIndexOf("error:"), before.lastIndexOf("error["));
    if (errorAt >= 0 && errorAt > warningAt) return match;
  }
  return null;
}

// Returns { file, line, test, message, excerpt } or null.
// Warning arrows are ignored so a manifest note is not reported as the failure.
export function firstFailure(raw) {
  const text = stripAnsi(raw);
  const patterns = [
    /panicked at (?<file>[^\s:'"]+):(?<line>\d+):\d+/,
    /panicked at '(?:[^'\\]|\\.)*', (?<file>[^\s:'"]+):(?<line>\d+):\d+/,
    /panicked at "(?:[^"\\]|\\.)*", (?<file>[^\s:'"]+):(?<line>\d+):\d+/,
  ];
  let match = null;
  for (const pattern of patterns) {
    match = pattern.exec(text);
    if (match?.groups) break;
  }
  if (!match?.groups) match = errorArrow(text);
  if (!match?.groups) return null;
  const file = cleanPath(match.groups.file);
  const line = match.groups.line;
  const before = text.slice(0, match.index);
  const named = [...before.matchAll(/---- ([^\s]+) stdout ----/g)].pop();
  const test = named?.[1] ?? "";
  const message = test
    ? `${test} failed at ${file}:${line}`
    : `failed at ${file}:${line}`;
  return { file, line, test, message, excerpt: excerptAround(text, match.index) };
}

export function failureTail(raw) {
  const text = stripAnsi(raw);
  const lines = text.split("\n");
  const errors = lines.filter((line) => /^\s*error(\[|:)/.test(line));
  const tail = lines.filter((line) => line.trim()).slice(-40);
  return [...errors.slice(0, 20), ...tail].filter((line, index, all) => all.indexOf(line) === index).join("\n");
}

export function reportFailure(raw, { summaryPath, write = console.log } = {}) {
  const found = firstFailure(raw);
  if (!found) {
    const lines = stripAnsi(raw).trim().split("\n");
    write(lines.slice(-30).join("\n"));
    return null;
  }
  write(`::error file=${found.file},line=${found.line}::${found.message}`);
  write(found.excerpt);
  if (summaryPath) {
    const body = [
      "### First failing test",
      "",
      `\`${found.test || found.message}\` at \`${found.file}:${found.line}\``,
      "",
      "```",
      found.excerpt,
      "```",
      "",
    ].join("\n");
    return { found, summary: body };
  }
  return { found, summary: "" };
}
