import { run } from './engine.mjs';
try {
  const changed = await run(undefined, true);
  if (changed.length) {
    console.error(`Freshness drift: ${changed.join(', ')}. Run node scripts/freshness/sync.mjs`);
    process.exitCode = 1;
  } else console.log('Freshness blocks are in sync');
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
