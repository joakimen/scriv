package config

import (
	"context"
	"os"
	"path/filepath"
	"reflect"
	"testing"
)

func writeConfig(t *testing.T, contents string) string {
	t.Helper()
	dir := t.TempDir()
	path := filepath.Join(dir, "config.json")
	if err := os.WriteFile(path, []byte(contents), 0o600); err != nil {
		t.Fatalf("writing config: %v", err)
	}
	return path
}

func TestLoadValid(t *testing.T) {
	path := writeConfig(t, `{
		"paths": [{"path": "~/dev", "depth": 2}],
		"ignore": ["foo", "bar"]
	}`)
	t.Setenv(configFileEnvVar, path)

	cfg, err := Load()
	if err != nil {
		t.Fatalf("Load: %v", err)
	}

	want := Config{
		Paths:  []PathEntry{{Path: "~/dev", Depth: 2}},
		Ignore: []string{"foo", "bar"},
	}
	if !reflect.DeepEqual(cfg, want) {
		t.Errorf("got %+v, want %+v", cfg, want)
	}
}

func TestLoadAppliesDefaultIgnore(t *testing.T) {
	path := writeConfig(t, `{"paths": [{"path": "~/dev"}]}`)
	t.Setenv(configFileEnvVar, path)

	cfg, err := Load()
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	if !reflect.DeepEqual(cfg.Ignore, defaultIgnoredDirs) {
		t.Errorf("expected default ignore list, got %v", cfg.Ignore)
	}
}

func TestLoadMissingFile(t *testing.T) {
	t.Setenv(configFileEnvVar, filepath.Join(t.TempDir(), "does-not-exist.json"))
	if _, err := Load(); err == nil {
		t.Fatal("expected error for missing file, got nil")
	}
}

func TestLoadInvalidJSON(t *testing.T) {
	path := writeConfig(t, `{not json`)
	t.Setenv(configFileEnvVar, path)
	if _, err := Load(); err == nil {
		t.Fatal("expected parse error, got nil")
	}
}

func TestFilePathHonorsEnv(t *testing.T) {
	t.Setenv(configFileEnvVar, "/tmp/custom.json")
	got, err := FilePath()
	if err != nil {
		t.Fatalf("FilePath: %v", err)
	}
	if got != "/tmp/custom.json" {
		t.Errorf("got %q, want /tmp/custom.json", got)
	}
}

func TestWithAndGetConfig(t *testing.T) {
	cfg := Config{Paths: []PathEntry{{Path: "/x", Depth: 1}}}
	ctx := WithConfig(context.Background(), cfg)
	got, err := GetConfig(ctx)
	if err != nil {
		t.Fatalf("GetConfig: %v", err)
	}
	if !reflect.DeepEqual(got, cfg) {
		t.Errorf("got %+v, want %+v", got, cfg)
	}
}

func TestGetConfigMissing(t *testing.T) {
	if _, err := GetConfig(context.Background()); err == nil {
		t.Fatal("expected error when config not in context")
	}
}
