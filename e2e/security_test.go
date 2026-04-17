package e2e

import (
	"context"
	"fmt"
	"io"
	"os"
	"strings"
	"testing"
	"time"

	batchv1 "k8s.io/api/batch/v1"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/client-go/kubernetes"
	"sigs.k8s.io/e2e-framework/klient/wait"
	"sigs.k8s.io/e2e-framework/pkg/envconf"
	"sigs.k8s.io/e2e-framework/pkg/features"
)

const (
	// hardenedToken must be ≥32 chars (enforced by the server at startup).
	hardenedToken = "hardened-token-minimum-32-characters-ok"
	standardToken = "standard-test-token"
)

// secCheck describes a single HTTP assertion: the expected status code and the
// optional value to pass as the Authorization header (empty → no header sent).
type secCheck struct {
	expectedCode string
	authValue    string // e.g. "Bearer mytoken", empty for unauthenticated
}

// buildCheckScript assembles a POSIX shell script that fires each check
// against the cache service using raw nc (netcat). The service is assumed to
// be reachable at cache:8080 within the pod's network.
func buildCheckScript(checks []secCheck) string {
	var b strings.Builder

	b.WriteString(`#!/bin/sh
set -e

# check_code <expected_http_code> <auth_value_or_empty>
check_code() {
  expected="$1"
  auth="$2"
  if [ -n "$auth" ]; then
    raw=$(printf 'GET /v1/cache/deadbeef00 HTTP/1.1\r\nHost: cache\r\nAuthorization: %s\r\nConnection: close\r\n\r\n' "$auth" \
      | nc -w 10 cache 8080 2>/dev/null | head -1)
  else
    raw=$(printf 'GET /v1/cache/deadbeef00 HTTP/1.1\r\nHost: cache\r\nConnection: close\r\n\r\n' \
      | nc -w 10 cache 8080 2>/dev/null | head -1)
  fi
  code=$(echo "$raw" | awk '{print $2}')
  if [ "$code" = "$expected" ]; then
    echo "PASS [auth=${auth:-none}]: expected $expected, got $code"
  else
    echo "FAIL [auth=${auth:-none}]: expected $expected, got $code (raw: $raw)"
    exit 1
  fi
}

`)

	for _, c := range checks {
		fmt.Fprintf(&b, "check_code %q %q\n", c.expectedCode, c.authValue)
	}
	b.WriteString(`echo "all security checks passed"` + "\n")
	return b.String()
}

// securityCheckJob creates a Job in the given namespace that runs the provided
// shell script using busybox (already loaded into the Kind cluster).
func securityCheckJob(ns, name, script string) *batchv1.Job {
	backoff := int32(0)
	return &batchv1.Job{
		ObjectMeta: metav1.ObjectMeta{Name: name, Namespace: ns},
		Spec: batchv1.JobSpec{
			BackoffLimit: &backoff,
			Template: corev1.PodTemplateSpec{
				Spec: corev1.PodSpec{
					RestartPolicy: corev1.RestartPolicyNever,
					InitContainers: []corev1.Container{
						{
							Name:            "wait-for-cache",
							Image:           busybox,
							ImagePullPolicy: corev1.PullNever,
							Command: []string{"sh", "-c",
								`until wget -qO- http://cache:8080/healthz; do echo "waiting for cache..."; sleep 2; done`,
							},
						},
					},
					Containers: []corev1.Container{
						{
							Name:            "check",
							Image:           busybox,
							ImagePullPolicy: corev1.PullNever,
							Command:         []string{"sh", "-c", script},
						},
					},
				},
			},
		},
	}
}

// runSecurityCheckJob creates the Job, waits for it to finish, and prints its
// logs regardless of outcome. Returns a fatal error if the job fails.
func runSecurityCheckJob(ctx context.Context, t *testing.T, cfg *envconf.Config, ns, name, script string) {
	t.Helper()
	job := securityCheckJob(ns, name, script)
	if err := cfg.Client().Resources().Create(ctx, job); err != nil {
		t.Fatalf("create security check job: %v", err)
	}
	err := wait.For(jobFinished(cfg, job), wait.WithTimeout(3*time.Minute))
	printJobLogs(ctx, t, cfg, ns, name, "check")
	if err != nil {
		t.Fatalf("security check job failed: %v", err)
	}
}

// printJobLogs streams logs from a named container in a named job to stdout.
func printJobLogs(ctx context.Context, t *testing.T, cfg *envconf.Config, ns, jobName, containerName string) {
	t.Helper()
	cs, err := kubernetes.NewForConfig(cfg.Client().RESTConfig())
	if err != nil {
		t.Logf("warning: could not build k8s clientset: %v", err)
		return
	}
	pods, err := cs.CoreV1().Pods(ns).List(ctx, metav1.ListOptions{
		LabelSelector: "job-name=" + jobName,
	})
	if err != nil || len(pods.Items) == 0 {
		t.Logf("warning: no pods for job %s: %v", jobName, err)
		return
	}
	for _, pod := range pods.Items {
		req := cs.CoreV1().Pods(ns).GetLogs(pod.Name, &corev1.PodLogOptions{
			Container: containerName,
		})
		stream, err := req.Stream(ctx)
		if err != nil {
			continue
		}
		defer stream.Close()
		fmt.Fprintf(os.Stdout, "\n=== %s / %s ===\n", pod.Name, containerName)
		io.Copy(os.Stdout, stream)
	}
}

// ── open ─────────────────────────────────────────────────────────────────────

// TestSecurityOpen verifies that the "open" level accepts requests without any
// Authorization header: a GET for a missing key must return 404, not 401/403.
func TestSecurityOpen(t *testing.T) {
	const (
		ns      = "sec-open"
		release = "nx-cache-sec-open"
	)

	feat := features.New("security: open level — no auth required").
		Setup(func(ctx context.Context, t *testing.T, cfg *envconf.Config) context.Context {
			if _, err := createNs(ctx, cfg, ns); err != nil {
				t.Fatalf("create namespace: %v", err)
			}
			if err := helmInstall(cfg, release, ns,
				"fullnameOverride=cache",
				"security.level=open",
				"storage.emptyDir=true",
			); err != nil {
				t.Fatalf("helm install: %v", err)
			}
			return ctx
		}).
		Assess("unauthenticated request is accepted (404, not 401/403)", func(ctx context.Context, t *testing.T, cfg *envconf.Config) context.Context {
			script := buildCheckScript([]secCheck{
				// No auth → 404 (cache miss; auth not required in open mode).
				{expectedCode: "404", authValue: ""},
			})
			runSecurityCheckJob(ctx, t, cfg, ns, "sec-check-open", script)
			return ctx
		}).
		Teardown(func(ctx context.Context, t *testing.T, cfg *envconf.Config) context.Context {
			_ = helmUninstall(cfg, release, ns)
			_, _ = deleteNs(ctx, cfg, ns)
			return ctx
		}).
		Feature()

	testenv.Test(t, feat)
}

// ── standard ─────────────────────────────────────────────────────────────────

// TestSecurityStandard verifies bearer-token enforcement at the "standard"
// level: missing token → 401, wrong token → 403, correct token → 404 (miss).
func TestSecurityStandard(t *testing.T) {
	const (
		ns      = "sec-standard"
		release = "nx-cache-sec-standard"
	)

	feat := features.New("security: standard level — bearer token required").
		Setup(func(ctx context.Context, t *testing.T, cfg *envconf.Config) context.Context {
			if _, err := createNs(ctx, cfg, ns); err != nil {
				t.Fatalf("create namespace: %v", err)
			}
			if err := helmInstall(cfg, release, ns,
				"fullnameOverride=cache",
				"security.level=standard",
				"security.token="+standardToken,
				"storage.emptyDir=true",
			); err != nil {
				t.Fatalf("helm install: %v", err)
			}
			return ctx
		}).
		Assess("token enforcement: 401 → 403 → 404", func(ctx context.Context, t *testing.T, cfg *envconf.Config) context.Context {
			script := buildCheckScript([]secCheck{
				{expectedCode: "401", authValue: ""},
				{expectedCode: "403", authValue: "Bearer wrong-token"},
				{expectedCode: "404", authValue: "Bearer " + standardToken},
			})
			runSecurityCheckJob(ctx, t, cfg, ns, "sec-check-standard", script)
			return ctx
		}).
		Teardown(func(ctx context.Context, t *testing.T, cfg *envconf.Config) context.Context {
			_ = helmUninstall(cfg, release, ns)
			_, _ = deleteNs(ctx, cfg, ns)
			return ctx
		}).
		Feature()

	testenv.Test(t, feat)
}

// ── hardened ─────────────────────────────────────────────────────────────────

// TestSecurityHardened verifies the same token-enforcement behaviour as
// standard but at the "hardened" level (constant-time comparison, ≥32-char
// token required). HTTP semantics are identical from the outside.
func TestSecurityHardened(t *testing.T) {
	const (
		ns      = "sec-hardened"
		release = "nx-cache-sec-hardened"
	)

	feat := features.New("security: hardened level — constant-time token comparison").
		Setup(func(ctx context.Context, t *testing.T, cfg *envconf.Config) context.Context {
			if _, err := createNs(ctx, cfg, ns); err != nil {
				t.Fatalf("create namespace: %v", err)
			}
			if err := helmInstall(cfg, release, ns,
				"fullnameOverride=cache",
				"security.level=hardened",
				"security.token="+hardenedToken,
				"storage.emptyDir=true",
			); err != nil {
				t.Fatalf("helm install: %v", err)
			}
			return ctx
		}).
		Assess("token enforcement: 401 → 403 → 404", func(ctx context.Context, t *testing.T, cfg *envconf.Config) context.Context {
			script := buildCheckScript([]secCheck{
				{expectedCode: "401", authValue: ""},
				{expectedCode: "403", authValue: "Bearer wrong-token"},
				{expectedCode: "404", authValue: "Bearer " + hardenedToken},
			})
			runSecurityCheckJob(ctx, t, cfg, ns, "sec-check-hardened", script)
			return ctx
		}).
		Teardown(func(ctx context.Context, t *testing.T, cfg *envconf.Config) context.Context {
			_ = helmUninstall(cfg, release, ns)
			_, _ = deleteNs(ctx, cfg, ns)
			return ctx
		}).
		Feature()

	testenv.Test(t, feat)
}

// ── paranoid ─────────────────────────────────────────────────────────────────

// TestSecurityParanoid verifies k8s TokenReview enforcement:
//   - no token              → 401
//   - invalid (non-JWT)     → 403
//   - valid SA token from the allowed namespace → 404 (auth passed, cache miss)
//
// The chart creates a ServiceAccount + ClusterRole/Binding so the server can
// call the TokenReview API. The test pod's projected SA token is used as the
// "valid caller" credential.
func TestSecurityParanoid(t *testing.T) {
	const (
		ns      = "sec-paranoid"
		release = "nx-cache-sec-paranoid"
	)

	feat := features.New("security: paranoid level — k8s TokenReview").
		Setup(func(ctx context.Context, t *testing.T, cfg *envconf.Config) context.Context {
			if _, err := createNs(ctx, cfg, ns); err != nil {
				t.Fatalf("create namespace: %v", err)
			}
			if err := helmInstall(cfg, release, ns,
				"fullnameOverride=cache",
				"security.level=paranoid",
				"serviceAccount.create=true",
				"security.allowedNamespaces="+ns,
				"storage.emptyDir=true",
			); err != nil {
				t.Fatalf("helm install: %v", err)
			}
			return ctx
		}).
		Assess("TokenReview enforcement: 401 → 403 → 404 with valid SA token", func(ctx context.Context, t *testing.T, cfg *envconf.Config) context.Context {
			// The test job reads its own projected SA token (from the allowed
			// namespace) and passes it as the Bearer credential for the final check.
			script := buildCheckScript([]secCheck{
				{expectedCode: "401", authValue: ""},
				{expectedCode: "403", authValue: "Bearer not-a-jwt"},
			}) +
				// Append a dynamic check using the pod's own SA token.
				`sa_token=$(cat /var/run/secrets/kubernetes.io/serviceaccount/token)
check_code "404" "Bearer $sa_token"
`
			runSecurityCheckJob(ctx, t, cfg, ns, "sec-check-paranoid", script)
			return ctx
		}).
		Teardown(func(ctx context.Context, t *testing.T, cfg *envconf.Config) context.Context {
			_ = helmUninstall(cfg, release, ns)
			_, _ = deleteNs(ctx, cfg, ns)
			return ctx
		}).
		Feature()

	testenv.Test(t, feat)
}
