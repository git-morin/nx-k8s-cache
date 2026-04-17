package e2e

import (
	"context"
	"fmt"
	"io"
	"os"
	"testing"
	"time"

	batchv1 "k8s.io/api/batch/v1"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/client-go/kubernetes"
	"sigs.k8s.io/e2e-framework/klient/decoder"
	"sigs.k8s.io/e2e-framework/klient/wait"
	"sigs.k8s.io/e2e-framework/klient/wait/conditions"
	"sigs.k8s.io/e2e-framework/pkg/envconf"
	"sigs.k8s.io/e2e-framework/pkg/features"
)

// TestCacheRoundTrip deploys the cache server using the raw Kubernetes
// manifests from e2e/manifests/ and verifies a full Nx remote-cache
// round-trip: first build populates the cache, second build restores from it.
func TestCacheRoundTrip(t *testing.T) {
	feat := features.New("nx remote cache round-trip (raw manifests)").
		Setup(func(ctx context.Context, t *testing.T, cfg *envconf.Config) context.Context {
			if err := createCacheSecret(ctx, cfg, namespace); err != nil {
				t.Fatalf("create cache secret: %v", err)
			}
			if err := decoder.ApplyWithManifestDir(ctx, cfg.Client().Resources(), manifests, "cache.yaml", nil); err != nil {
				t.Fatalf("apply cache.yaml: %v", err)
			}
			if err := wait.For(
				conditions.New(cfg.Client().Resources()).DeploymentAvailable("cache", namespace),
				wait.WithTimeout(2*time.Minute),
			); err != nil {
				t.Fatalf("cache deployment not ready: %v", err)
			}
			return ctx
		}).
		Assess("runner job completes and confirms remote cache hit", func(ctx context.Context, t *testing.T, cfg *envconf.Config) context.Context {
			return runRunnerJob(ctx, t, cfg, namespace)
		}).
		Teardown(func(ctx context.Context, t *testing.T, cfg *envconf.Config) context.Context {
			_ = decoder.DeleteWithManifestDir(ctx, cfg.Client().Resources(), manifests, "runner.yaml", nil)
			_ = decoder.DeleteWithManifestDir(ctx, cfg.Client().Resources(), manifests, "cache.yaml", nil)
			secret := &corev1.Secret{ObjectMeta: metav1.ObjectMeta{Name: "cache-secret", Namespace: namespace}}
			_ = cfg.Client().Resources().Delete(ctx, secret)
			return ctx
		}).
		Feature()

	testenv.Test(t, feat)
}

// TestCacheRoundTripHelm deploys the cache server using the Helm chart and
// verifies the same round-trip. This test exercises the chart on top of the
// raw-manifest test to validate both deployment methods.
func TestCacheRoundTripHelm(t *testing.T) {
	const release = "nx-cache-helm"
	const helmNs = "e2e-helm"

	feat := features.New("nx remote cache round-trip (Helm chart)").
		Setup(func(ctx context.Context, t *testing.T, cfg *envconf.Config) context.Context {
			if _, err := createNs(ctx, cfg, helmNs); err != nil {
				t.Fatalf("create namespace: %v", err)
			}
			if err := createCacheSecret(ctx, cfg, helmNs); err != nil {
				t.Fatalf("create cache secret: %v", err)
			}
			if err := helmInstall(cfg, release, helmNs,
				"fullnameOverride=cache",
				"security.level=standard",
				"security.token="+cacheToken,
			); err != nil {
				t.Fatalf("helm install: %v", err)
			}
			return ctx
		}).
		Assess("runner job completes and confirms remote cache hit", func(ctx context.Context, t *testing.T, cfg *envconf.Config) context.Context {
			return runRunnerJob(ctx, t, cfg, helmNs)
		}).
		Teardown(func(ctx context.Context, t *testing.T, cfg *envconf.Config) context.Context {
			_ = decoder.DeleteWithManifestDir(ctx, cfg.Client().Resources(), manifests, "runner.yaml", nil)
			_ = helmUninstall(cfg, release, helmNs)
			_, _ = deleteNs(ctx, cfg, helmNs)
			return ctx
		}).
		Feature()

	testenv.Test(t, feat)
}

// runRunnerJob applies runner.yaml in the given namespace, waits for the Job
// to complete, and prints runner logs. Shared by both round-trip tests.
func runRunnerJob(ctx context.Context, t *testing.T, cfg *envconf.Config, ns string) context.Context {
	t.Helper()

	// runner.yaml hard-codes namespace "e2e"; patch on the fly if needed.
	if err := applyRunnerJob(ctx, cfg, ns); err != nil {
		t.Fatalf("apply runner job: %v", err)
	}

	job := &batchv1.Job{ObjectMeta: metav1.ObjectMeta{Name: "runner", Namespace: ns}}
	err := wait.For(jobFinished(cfg, job), wait.WithTimeout(10*time.Minute))
	printRunnerLogs(ctx, t, cfg, ns)
	if err != nil {
		t.Fatalf("runner job did not succeed: %v", err)
	}
	return ctx
}

// applyRunnerJob creates the runner Job directly (so we can set the namespace
// dynamically) rather than relying on the namespace hard-coded in runner.yaml.
func applyRunnerJob(ctx context.Context, cfg *envconf.Config, ns string) error {
	return decoder.ApplyWithManifestDir(ctx, cfg.Client().Resources(), manifests, "runner.yaml", nil,
		decoder.MutateNamespace(ns),
	)
}

// jobFinished is a wait condition that resolves true on JobComplete and
// returns a hard error on JobFailed, avoiding an unnecessary timeout wait.
func jobFinished(cfg *envconf.Config, job *batchv1.Job) func(ctx context.Context) (bool, error) {
	return func(ctx context.Context) (bool, error) {
		current := &batchv1.Job{}
		if err := cfg.Client().Resources().Get(ctx, job.Name, job.Namespace, current); err != nil {
			return false, err
		}
		for _, cond := range current.Status.Conditions {
			switch cond.Type {
			case batchv1.JobFailed:
				if cond.Status == corev1.ConditionTrue {
					return false, fmt.Errorf("runner job failed: %s", cond.Message)
				}
			case batchv1.JobComplete:
				if cond.Status == corev1.ConditionTrue {
					return true, nil
				}
			}
		}
		return false, nil
	}
}

// printRunnerLogs streams logs from every container in each runner pod to
// stdout. Called both on success and failure so CI always has a trace.
func printRunnerLogs(ctx context.Context, t *testing.T, cfg *envconf.Config, ns string) {
	t.Helper()

	cs, err := kubernetes.NewForConfig(cfg.Client().RESTConfig())
	if err != nil {
		t.Logf("warning: could not build k8s clientset for log streaming: %v", err)
		return
	}

	pods, err := cs.CoreV1().Pods(ns).List(ctx, metav1.ListOptions{
		LabelSelector: "job-name=runner",
	})
	if err != nil || len(pods.Items) == 0 {
		t.Logf("warning: no runner pods found: %v", err)
		return
	}

	for _, pod := range pods.Items {
		for _, container := range []string{"wait-for-cache", "runner"} {
			req := cs.CoreV1().Pods(ns).GetLogs(pod.Name, &corev1.PodLogOptions{
				Container: container,
			})
			stream, err := req.Stream(ctx)
			if err != nil {
				continue
			}
			defer stream.Close()
			fmt.Fprintf(os.Stdout, "\n=== %s / %s ===\n", pod.Name, container)
			io.Copy(os.Stdout, stream)
		}
	}
}
