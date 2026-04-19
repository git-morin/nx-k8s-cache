# Contributing

## Prerequisites

- [Rust](https://rustup.rs) (stable)
- [Docker](https://docs.docker.com/get-docker/)
- [Go 1.23+](https://go.dev/dl/) (for e2e tests)
- [Helm](https://helm.sh/docs/intro/install/) (for e2e tests)

## Running the server locally

```bash
NX_CACHE_TOKEN=secret cargo run
```

The server starts on `http://localhost:8080`.

## Running unit tests

```bash
cargo test
```

The integration tests in `unit` CI spin up the server against a real Nx workspace. You can replicate this locally by following the steps in `.github/workflows/ci.yml`.

## Running e2e tests

The e2e suite requires Docker and a working `kubectl` context (kind or a real cluster).

```bash
cd e2e
go mod tidy
go test ./... -v -count=1 -timeout 20m
```

## Branches

| Branch | Purpose |
| --- | --- |
| `main` | Stable releases |
| `dev` | Preview releases (`-dev.N`) |

Work on feature branches cut from `main`. Open a PR targeting `main`.

## Commits

Follow [Conventional Commits](https://www.conventionalcommits.org/). This drives semantic versioning:

| Prefix | Effect |
| --- | --- |
| `fix:` | Patch release |
| `feat:` | Minor release |
| `feat!:` / `BREAKING CHANGE` | Major release |
| `chore:`, `ci:`, `docs:` | No release |

## Submitting a PR

1. Fork the repo and create a branch from `main`.
2. Make sure `cargo test` passes.
3. Open a PR. CI runs unit and e2e tests automatically.
4. A maintainer will review and squash-merge once approved.
