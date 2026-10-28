import { run } from './engine.mjs';
try {
  const changed = await run();
  console.log(`Freshness synced: ${changed.length} file(s)`);
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
