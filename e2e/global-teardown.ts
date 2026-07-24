import { execFileSync } from 'node:child_process';
import path from 'node:path';

// Purge the e2e-* players/games this run created (see scripts/e2e-clean.sh).
// Best-effort: a cleanup failure (e.g. the script isn't reachable in CI)
// must never fail an otherwise-green suite.
export default async function globalTeardown() {
  try {
    execFileSync(path.join(__dirname, '..', 'scripts', 'e2e-clean.sh'), {
      stdio: 'inherit',
    });
  } catch (err) {
    console.warn('[e2e] cleanup skipped:', (err as Error).message);
  }
}
