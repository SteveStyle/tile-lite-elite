# End-to-end UI tests (Playwright)

Browser tests that drive the real (wasm) web client against a running backend —
catching client/server **contract** breaks that Rust unit/integration tests
can't see, since those never render the client or cross the wire. This is the
only Node in the repo, quarantined to this directory and kept out of the cargo
workspace.

## Running

The suite runs against an **already-running dev environment** — it does not
start the app itself. That's [runbook](../docs/3.3-testing-ci-and-release.md#shipping-a-change-the-full-sequence)
step 2 (`./scripts/services.sh restart-server` or `restart`), which brings up
the server (`:3000`) and web client (`:8080`). Then:

```bash
cd e2e
npm install            # first time only
npx playwright install chromium   # first time only — downloads the browser
npm test               # runs the suite against http://localhost:8080
npm run test:headed    # watch it in a real browser window
npm run report         # open the last HTML report
```

Point it elsewhere (e.g. staging) with `PLAYWRIGHT_BASE_URL=http://localhost:8081 npm test`.

## Test data & cleanup

Every player a test creates is named with the `e2e-` prefix and a per-run
suffix, so runs never collide and the data is always identifiable as
disposable. The prefix is the whole contract behind cleanup:
[`scripts/e2e-clean.sh`](../scripts/e2e-clean.sh) removes all `e2e-*` players
and the games they played, plus dependent rows. It runs automatically as the
Playwright global teardown, and can be run by hand (`npm run clean`) any time.

## Layout

- `playwright.config.ts` — Chromium, `baseURL` from `PLAYWRIGHT_BASE_URL`, no `webServer` (the app is external).
- `global-teardown.ts` — best-effort call to `scripts/e2e-clean.sh`.
- `tests/helpers.ts` — auth flows (register/login/logout) and the `e2e-` naming.
- `tests/smoke.spec.ts` — the first suite: register, login/logout, stay-logged-in, and Play-Greedy-Bot-renders-a-board (the flow whose skew bug prompted this suite).
