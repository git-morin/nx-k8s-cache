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

func TestCacheRoundTrip(t *testing.T) {
	feat := features.New("nx remote cache round-trip").
		Setup(func(ctx context.Context, t *testing.T, cfg *envconf.Config) context.Context {
			// Block until the cache Deployment is Available before running any test.
			err := wait.For(
				conditions.New(cfg.Client().Resources()).DeploymentAvailable("cache", namespace),
				wait.WithTimeout(2*time.Minute),
			)
			if err != nil {
				t.Fatalf("cache deployment not ready: %v", err)
			}
			return ctx
		}).
		Assess("runner job completes and confirms remote cache hit", func(ctx context.Context, t *testing.T, cfg *envconf.Config) context.Context {
			if err := decoder.ApplyWithManifestDir(ctx, cfg.Client().Resources(), manifests, "runner.yaml", nil); err != nil {
				t.Fatalf("apply runner.yaml: %v", err)
			}

			job := &batchv1.Job{ObjectMeta: metav1.ObjectMeta{Name: "runner", Namespace: namespace}}

			// Wait for the job to finish. jobFinished fails fast on job failure
			// rather than waiting for the full timeout to expire.
			err := wait.For(
				jobFinished(cfg, job),
				wait.WithTimeout(10*time.Minute),
			)

			// Print logs regardless of outcome so CI always has the output.
			printRunnerLogs(ctx, t, cfg)

			if err != nil {
				t.Fatalf("runner job did not succeed: %v", err)
			}
			return ctx
		}).
		Teardown(func(ctx context.Context, t *testing.T, cfg *envconf.Config) context.Context {
			_ = decoder.DeleteWithManifestDir(ctx, cfg.Client().Resources(), manifests, "runner.yaml", nil)
			return ctx
		}).
		Feature()

	testenv.Test(t, feat)
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
func printRunnerLogs(ctx context.Context, t *testing.T, cfg *envconf.Config) {
	t.Helper()

	cs, err := kubernetes.NewForConfig(cfg.Client().RESTConfig())
	if err != nil {
		t.Logf("warning: could not build k8s clientset for log streaming: %v", err)
		return
	}

	pods, err := cs.CoreV1().Pods(namespace).List(ctx, metav1.ListOptions{
		LabelSelector: "job-name=runner",
	})
	if err != nil || len(pods.Items) == 0 {
		t.Logf("warning: no runner pods found: %v", err)
		return
	}

	for _, pod := range pods.Items {
		for _, container := range []string{"wait-for-cache", "runner"} {
			req := cs.CoreV1().Pods(namespace).GetLogs(pod.Name, &corev1.PodLogOptions{
				Container: container,
			})
			stream, err := req.Stream(ctx)
			if err != nil {
				continue // container may not have started
			}
			defer stream.Close()
			fmt.Fprintf(os.Stdout, "\n=== %s / %s ===\n", pod.Name, container)
			io.Copy(os.Stdout, stream)
		}
	}
}
