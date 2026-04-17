package e2e

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"testing"
	"time"

	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/e2e-framework/pkg/env"
	"sigs.k8s.io/e2e-framework/pkg/envconf"
	"sigs.k8s.io/e2e-framework/pkg/envfuncs"
	"sigs.k8s.io/e2e-framework/support/kind"
)

const (
	clusterName    = "nx-cache-e2e"
	namespace      = "e2e"
	cacheImageRepo = "nx-cache-server"
	cacheImageTag  = "ci"
	cacheImage     = cacheImageRepo + ":" + cacheImageTag
	runnerImage    = "nx-runner:ci"
	busybox        = "busybox:stable"
	cacheToken     = "ci-test-token"
	manifests      = "manifests"
)

var testenv env.Environment

// helmChart returns the absolute path to the Helm chart, resolved relative to
// this source file so it works regardless of the test working directory.
func helmChart() string {
	_, file, _, _ := runtime.Caller(0)
	return filepath.Join(filepath.Dir(file), "..", "deploy", "helm", "nx-k8s-cache")
}

// helmInstall runs `helm install` for the cache chart with CI image overrides
// already baked in. Extra key=value pairs can be appended via sets.
func helmInstall(cfg *envconf.Config, release, ns string, sets ...string) error {
	args := []string{
		"install", release, helmChart(),
		"--namespace", ns,
		"--kubeconfig", cfg.KubeconfigFile(),
		"--set", "image.repository=" + cacheImageRepo,
		"--set", "image.tag=" + cacheImageTag,
		"--set", "image.pullPolicy=Never",
		"--set", "storage.emptyDir=true",
		"--wait",
		"--timeout", "2m",
	}
	for _, s := range sets {
		args = append(args, "--set", s)
	}
	cmd := exec.Command("helm", args...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	return cmd.Run()
}

// helmUninstall removes a Helm release.
func helmUninstall(cfg *envconf.Config, release, ns string) error {
	cmd := exec.Command("helm", "uninstall", release,
		"--namespace", ns,
		"--kubeconfig", cfg.KubeconfigFile(),
	)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	return cmd.Run()
}

// waitForDeployment polls until the named Deployment has at least one
// available replica or the timeout elapses.
func waitForDeployment(ctx context.Context, cfg *envconf.Config, name, ns string) error {
	deadline := time.Now().Add(2 * time.Minute)
	for time.Now().Before(deadline) {
		d := &appsv1.Deployment{}
		if err := cfg.Client().Resources().Get(ctx, name, ns, d); err == nil {
			if d.Status.AvailableReplicas > 0 {
				return nil
			}
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(2 * time.Second):
		}
	}
	return fmt.Errorf("deployment %s/%s never became available", ns, name)
}

// createCacheSecret creates the token Secret used by raw-manifest deployments.
func createCacheSecret(ctx context.Context, cfg *envconf.Config, ns string) error {
	secret := &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{Name: "cache-secret", Namespace: ns},
		StringData: map[string]string{"token": cacheToken},
	}
	return cfg.Client().Resources().Create(ctx, secret)
}

// createNs and deleteNs are thin wrappers around the envfuncs helpers for use
// inside regular functions (as opposed to testenv.Setup/Finish chains).
func createNs(ctx context.Context, cfg *envconf.Config, ns string) (context.Context, error) {
	return envfuncs.CreateNamespace(ns)(ctx, cfg)
}

func deleteNs(ctx context.Context, cfg *envconf.Config, ns string) (context.Context, error) {
	return envfuncs.DeleteNamespace(ns)(ctx, cfg)
}

func TestMain(m *testing.M) {
	cfg, _ := envconf.NewFromFlags()
	testenv = env.NewWithConfig(cfg)

	testenv.Setup(
		envfuncs.CreateClusterWithConfig(
			kind.NewProvider(),
			clusterName,
			manifests+"/kind.yaml",
			kind.WithImage("kindest/node:v1.31.0"),
		),
		envfuncs.LoadImageToCluster(clusterName, cacheImage),
		envfuncs.LoadImageToCluster(clusterName, runnerImage),
		envfuncs.LoadImageToCluster(clusterName, busybox),
		envfuncs.CreateNamespace(namespace),
	)

	testenv.Finish(
		envfuncs.DeleteNamespace(namespace),
		envfuncs.DestroyCluster(clusterName),
	)

	os.Exit(testenv.Run(m))
}
