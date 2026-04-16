package e2e

import (
	"context"
	"os"
	"testing"

	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"sigs.k8s.io/e2e-framework/klient/decoder"
	"sigs.k8s.io/e2e-framework/pkg/env"
	"sigs.k8s.io/e2e-framework/pkg/envconf"
	"sigs.k8s.io/e2e-framework/pkg/envfuncs"
	"sigs.k8s.io/e2e-framework/support/kind"
)

const (
	clusterName = "nx-cache-e2e"
	namespace   = "e2e"
	cacheImage  = "nx-cache-server:ci"
	runnerImage = "nx-runner:ci"
	busybox     = "busybox:stable"
	cacheToken  = "ci-test-token"
	manifests   = "../k8s/e2e"
)

var testenv env.Environment

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
		createCacheSecret,
		deployCacheServer,
	)

	testenv.Finish(
		envfuncs.DeleteNamespace(namespace),
		envfuncs.DestroyCluster(clusterName),
	)

	os.Exit(testenv.Run(m))
}

// createCacheSecret injects the bearer token the cache server will validate.
func createCacheSecret(ctx context.Context, cfg *envconf.Config) (context.Context, error) {
	secret := &corev1.Secret{
		ObjectMeta: metav1.ObjectMeta{
			Name:      "cache-secret",
			Namespace: namespace,
		},
		StringData: map[string]string{"token": cacheToken},
	}
	return ctx, cfg.Client().Resources().Create(ctx, secret)
}

// deployCacheServer applies the cache Deployment and Service from cache.yaml.
func deployCacheServer(ctx context.Context, cfg *envconf.Config) (context.Context, error) {
	err := decoder.ApplyWithManifestDir(ctx, cfg.Client().Resources(), manifests, "cache.yaml", nil)
	return ctx, err
}
