# Crabcloud E2E (Playwright)

End-to-end tests for the Crabcloud HTTP surface, including real WASM hydration
in a headless Chromium.

## Prerequisites

- Node 20+
- `npm ci` (or `pnpm install`)
- `npx playwright install --with-deps chromium` (one-time)

## Running locally

In one terminal, start the server with a test config that includes
`bootstrap_admin`:

```bash
# Generate a bcrypt hash for "hunter2"
python3 -c "import bcrypt; print(bcrypt.hashpw(b'hunter2', bcrypt.gensalt(12)).decode())"

# Edit config/config.toml.example into a fixture with bind_address = "127.0.0.1:18765"
# and a [bootstrap_admin] section using the hash above.

cargo xtask build
cargo run --release -p crabcloud-server -- --config fixture.toml migrate
cargo run --release -p crabcloud-server -- --config fixture.toml serve
```

In another terminal:

```bash
cd e2e
npm test
```

## CI

The `e2e` job in `.github/workflows/ci.yml` automates the above: builds the
release binary, starts it on `127.0.0.1:18765` with a fixture config, runs
the Playwright tests, then tears down the server.

## Documentation screenshots

`screenshots.ts` (next to this README, not under `tests/`) drives the Files
UI through four states and saves PNGs into `docs/screenshots/`. It uses its
own Playwright config so it's not picked up by `npm test`.

Quick run (with the server already up at `127.0.0.1:18765`):

```bash
# From the repo root, in another terminal — start the server with the
# dedicated screenshots fixture (datadirectory is gitignored as
# screenshots-work/).
cargo run --release -p crabcloud-server -- --config config/screenshots.toml migrate
cargo run --release -p crabcloud-server -- --config config/screenshots.toml serve

# In e2e/:
npm run screenshots
```

Output:

- `docs/screenshots/files-empty.png`
- `docs/screenshots/files-list.png`
- `docs/screenshots/files-selection.png`
- `docs/screenshots/files-delete-modal.png`

The fixture's bootstrap admin is `admin` / `hunter2`.
