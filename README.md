# nx-k8s-cache

A self-hosted remote cache server for [Nx](https://nx.dev) monorepos, built with Rust and designed to run in Kubernetes.

> **Scope:** this project is intended for internal CI pipelines on trusted private networks. If you need a cache server exposed to the public internet or shared across untrusted tenants, use [Nx Cloud](https://nx.app) instead.

## How it works

Nx 20.8+ supports custom remote cache servers via a simple HTTP API. This server implements that spec: tasks are stored and retrieved by hash, protected by a bearer token.

## Usage

### Nx workspace

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

### Local development

```bash
NX_CACHE_TOKEN=<secret> cargo run
```

## Configuration

| Variable                  | Default         | Description                                                   |
| ------------------------- | --------------- | ------------------------------------------------------------- |
| `NX_CACHE_TOKEN`          | required*       | Bearer token Nx clients must present                          |
| `NX_CACHE_DIR`            | `/var/cache/nx` | Directory where cache artifacts are stored                    |
| `NX_CACHE_BACKEND`        | `disk`          | Storage backend: `disk` or `s3`                               |
| `NX_S3_BUCKET`            | required for s3 | S3 bucket name                                                |
| `NX_S3_ENDPOINT`          |                 | Custom S3 endpoint (e.g. MinIO)                               |
| `NX_S3_REGION`            |                 | AWS region                                                    |
| `NX_S3_PREFIX`            |                 | Key prefix for all objects                                    |
| `NX_CACHE_SECURITY_LEVEL` | `standard`      | Security level: `open`, `standard`, `hardened`, `paranoid`    |
| `NX_ALLOWED_NAMESPACES`   |                 | Comma-separated list of allowed namespaces (paranoid only)    |
| `NX_MAX_BODY_MB`          | `512`           | Maximum upload size in MiB                                    |
| `LOG_FORMAT`              | text            | Set to `json` for structured output                           |

\* Optional at level `open`, required at all other levels.

**S3:** when `NX_CACHE_BACKEND=s3`, credentials are read from standard AWS environment variables (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`) or a pod workload identity.

## Security levels

| Level          | Auth                  | Write-once | SHA-256 integrity | Rate limiting            |
| -------------- | --------------------- | ---------- | ----------------- | ------------------------ |
| `open` (0)     | none                  |            |                   |                          |
| `standard` (1) | bearer token          | yes        |                   |                          |
| `hardened` (2) | bearer, constant-time | yes        | yes               |                          |
| `paranoid` (3) | k8s SA token review   | yes        | yes               | 10 failures / 60 s / IP  |

**open:** no authentication, artifacts can be overwritten. Only appropriate for local development or fully trusted isolated networks.

**standard:** bearer token required, each hash can only be written once. Suitable for internal CI on a private cluster.

**hardened:** constant-time token comparison, SHA-256 sidecar stored on every PUT and verified on every GET (detects bit-rot and filesystem tampering), Content-Length validated. Suitable for shared or multi-team infrastructure.

**paranoid:** k8s service account tokens validated via the TokenReview API, namespace allowlist enforced, and per-IP rate limiting on auth failures. Requires running inside a Kubernetes cluster.

## Helm

The chart is located at `deploy/helm/nx-k8s-cache`. Install it directly from the repo:

```bash
helm install nx-k8s-cache ./deploy/helm/nx-k8s-cache \
  --set security.token=<your-token>
```

Key values:

| Value | Default | Description |
| --- | --- | --- |
| `security.level` | `standard` | Security level (see table above) |
| `security.token` | `""` | Bearer token (or use `existingSecret`) |
| `storage.backend` | `disk` | `disk` or `s3` |
| `storage.size` | `10Gi` | PVC size (disk backend) |
| `storage.emptyDir` | `false` | Use emptyDir instead of PVC (non-persistent) |
| `s3.bucket` | `""` | S3 bucket name (s3 backend) |
| `serviceAccount.create` | `false` | Required when using `paranoid` security level |

See [`deploy/helm/nx-k8s-cache/values.yaml`](deploy/helm/nx-k8s-cache/values.yaml) for the full reference.

## API

| Method | Path               | Description                                      |
| ------ | ------------------ | ------------------------------------------------ |
| `PUT`  | `/v1/cache/{hash}` | Store a task artifact                            |
| `GET`  | `/v1/cache/{hash}` | Retrieve a task artifact                         |
| `GET`  | `/healthz`         | Liveness probe                                   |
| `GET`  | `/readyz`          | Readiness probe (checks cache dir is accessible) |
| `GET`  | `/metrics`         | Prometheus metrics                               |

All cache endpoints require `Authorization: Bearer <token>` at security level `standard` and above.
