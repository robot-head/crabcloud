# Rustcloud E2E (Playwright)

End-to-end tests for the Rustcloud HTTP surface, including real WASM hydration
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
cargo run --release -p rustcloud-server -- --config fixture.toml migrate
cargo run --release -p rustcloud-server -- --config fixture.toml serve
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
