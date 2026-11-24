// Per-OS gate. A path-gate skip is success. Failure and cancellation are not.
import { fileURLToPath } from "node:url";

export function gateProblems(needs) {
  return Object.entries(needs)
    .filter(([, value]) => !value || (value.result !== "success" && value.result !== "skipped"))
    .map(([key, value]) => `${key}=${value?.result ?? "missing"}`);
}

function main() {
  const name = process.env.GATE_NAME || "OS gate";
  let needs;
  try {
    needs = JSON.parse(process.env.NEEDS || "");
  } catch {
    console.log(`::error::${name} could not read needs`);
    process.exit(1);
  }
  const bad = gateProblems(needs);
  if (bad.length) {
    console.log(`::error::${name}: ${bad.join(" ")}`);
    process.exit(1);
  }
  console.log(`${name} passed`);
}

const isMain = process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1];
if (isMain) main();
