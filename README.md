# Rustcloud

A Rust port of [Nextcloud server](https://github.com/nextcloud/server), with a Dioxus frontend.

**Status:** very early. See `docs/superpowers/specs/` for design specs and `docs/superpowers/plans/` for implementation plans.

## Quick start (development)

```bash
# Start dev databases
docker compose -f dev/docker-compose.yml up -d

# Build the workspace
cargo build

# Run lint + tests
cargo xtask check-all
```

## License

AGPL-3.0-or-later — see `LICENSE`.
