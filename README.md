# nx-k8s-cache

A self-hosted remote cache server for [Nx](https://nx.dev) monorepos, built with Rust and designed to run in Kubernetes.

## How it works

Nx 20.8+ supports custom remote cache servers via a simple HTTP API. This server implements that spec — tasks are stored and retrieved by hash, protected by a bearer token.

## Usage

### Nx workspace configuration

```bash
NX_SELF_HOSTED_REMOTE_CACHE_SERVER=http://<host>:8080
NX_SELF_HOSTED_REMOTE_CACHE_ACCESS_TOKEN=<your-token>
```

### Docker

```bash
docker run -e NX_CACHE_TOKEN=secret -v /data/cache:/cache \
  -e NX_CACHE_DIR=/cache -p 8080:8080 \
  ghcr.io/git-morin/nx-k8s-cache:latest
```

### Kubernetes

Apply the manifests from `e2e/manifests/` as a starting point, then create a secret with your token:

```bash
kubectl create secret generic cache-secret --from-literal=token=<your-token>
```

## Configuration

| Variable | Default | Description |
|---|---|---|
| `NX_CACHE_TOKEN` | required¹ | Bearer token Nx clients must present |
| `NX_CACHE_DIR` | `/var/cache/nx` | Directory where cache artifacts are stored |
| `NX_CACHE_SECURITY_LEVEL` | `standard` | Security level: `open`, `standard`, `hardened`, `paranoid` |
| `NX_MAX_BODY_MB` | `512` | Maximum upload size in MiB |
| `LOG_FORMAT` | text | Set to `json` for structured output |

¹ Optional at level `open`, required at all other levels.

## Security levels

| Level | Auth | Write-once | SHA-256 integrity | Rate limiting |
|---|---|---|---|---|
| `open` (0) | none | | | |
| `standard` (1) | bearer token | ✓ | | |
| `hardened` (2) | bearer, constant-time | ✓ | ✓ | |
| `paranoid` (3) | bearer, constant-time | ✓ | ✓ | 10 failures / 60 s / IP |

**open** — no authentication, artifacts can be overwritten. For local development or fully trusted private networks.

**standard** — bearer token required, each hash can only be written once. Suitable for internal CI.

**hardened** — constant-time token comparison (timing-safe), SHA-256 sidecar stored on every PUT and verified on every GET (detects bit-rot and filesystem tampering), Content-Length validated. Suitable for shared or multi-team infrastructure.

**paranoid** — everything in hardened plus a minimum token length of 32 characters and per-IP rate limiting on auth failures. Suitable for internet-facing deployments. Token minimum is enforced at startup.

## API

| Method | Path | Description |
|---|---|---|
| `PUT` | `/v1/cache/{hash}` | Store a task artifact |
| `GET` | `/v1/cache/{hash}` | Retrieve a task artifact |
| `GET` | `/healthz` | Liveness probe |
| `GET` | `/readyz` | Readiness probe (checks cache dir is accessible) |
| `GET` | `/metrics` | Prometheus metrics |

All cache endpoints require `Authorization: Bearer <token>` at security level `standard` and above.
