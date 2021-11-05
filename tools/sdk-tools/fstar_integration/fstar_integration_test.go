// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package fstar_integration

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"os/signal"
	"os/user"
	"path/filepath"
	"strings"
	"syscall"
	"testing"

	"github.com/google/go-cmp/cmp"
	"go.fuchsia.dev/fuchsia/tools/botanist/constants"
	"go.fuchsia.dev/fuchsia/tools/lib/ffxutil"
	"go.fuchsia.dev/fuchsia/tools/sdk-tools/sdkcommon"
)

type toolsPath struct {
	ffxPath       string
	ffxInstance   *ffxutil.FFXInstance
	ffxConfigPath string // Used to make ffx isolated.
	fconfigPath   string
	fsshPath      string
}

// findSubDir searches the root dir and returns the first path that matches dirName.
func findSubDir(root, dirName string) string {
	filePath := ""
	filepath.WalkDir(root,
		func(path string, info os.DirEntry, err error) error {
			if err != nil {
				return err
			}
			if info.IsDir() && info.Name() == dirName {
				filePath = path
			}
			return nil
		})
	return filePath
}

// setUp sets up the environment, finds the paths for the host tools, and creates
// an isolated ffx instance.
func setUp(t *testing.T) (toolsPath, error) {
	var tools toolsPath
	ex, err := os.Executable()
	if err != nil {
		return tools, err
	}

	exDir := filepath.Dir(ex)
	runtimeDir := findSubDir(exDir, "fstar_runtime_deps")
	if len(runtimeDir) == 0 {
		return tools, fmt.Errorf("cannot find fstar_runtime_deps binary from %s", exDir)
	}

	hostToolsDir := filepath.Join(runtimeDir, "host_tools")
	if _, err := os.Stat(hostToolsDir); os.IsNotExist(err) {
		return tools, err
	}

	tools.ffxPath = filepath.Join(hostToolsDir, "ffx")
	if _, err := os.Stat(tools.ffxPath); os.IsNotExist(err) {
		return tools, fmt.Errorf("unable to find ffx: %w", err)
	}

	tools.fsshPath = filepath.Join(hostToolsDir, "fssh")
	if _, err := os.Stat(tools.fsshPath); os.IsNotExist(err) {
		return tools, fmt.Errorf("unable to find fssh: %w", err)
	}

	tools.fconfigPath = filepath.Join(hostToolsDir, "fconfig")
	if _, err := os.Stat(tools.fconfigPath); os.IsNotExist(err) {
		return tools, fmt.Errorf("unable to find fconfig: %w", err)
	}

	testOutDir := filepath.Join(t.TempDir(), "fssh_test")

	// Create a new isolated ffx instance.
	tools.ffxInstance, err = ffxutil.NewFFXInstance(tools.ffxPath, "", os.Environ(), os.Getenv(constants.NodenameEnvKey), os.Getenv(constants.SSHKeyEnvKey), testOutDir)
	if err != nil {
		return tools, fmt.Errorf("unable to create new ffx instance %w", err)
	}

	tools.ffxConfigPath = filepath.Join(testOutDir, "ffx_config.json")
	if _, err := os.Stat(tools.ffxConfigPath); os.IsNotExist(err) {
		return tools, fmt.Errorf("ffx config path does not exist: %v", err)
	}

	return tools, nil
}

func TestFSSH(t *testing.T) {
	// In order to make sure the test cleans up the ffx instance, we capture SIGTERM until
	// we ensured the ffx instance is cleaned up.
	ctx, cancel := signal.NotifyContext(context.Background(), syscall.SIGTERM, syscall.SIGINT)
	defer cancel()

	tools, err := setUp(t)
	t.Cleanup(func() {
		os.Unsetenv(sdkcommon.FFXIsolatedEnvKey)
		if tools.ffxInstance != nil {
			tools.ffxInstance.Stop()
		}
	})

	if err != nil {
		t.Fatalf("unable to setup environment: %s", err)
	}

	deviceName := os.Getenv(constants.NodenameEnvKey)

	var stdoutBuf []byte
	stdout := bytes.NewBuffer(stdoutBuf)
	var stderrBuf []byte
	stderr := bytes.NewBuffer(stderrBuf)
	tools.ffxInstance.SetStdoutStderr(stdout, stderr)

	// Get the device IP.
	t.Logf("Getting the device IP")
	err = tools.ffxInstance.List(ctx, "--format", "a", deviceName)
	if err != nil {
		t.Fatalf("ffx target list returned unexpected error: %s", err)
	}

	ffxTargetListStderr := strings.TrimSpace(stderr.String())
	deviceIP := strings.TrimSpace(stdout.String())

	os.Setenv(sdkcommon.FFXIsolatedEnvKey, tools.ffxConfigPath)

	if ffxTargetListStderr != "" {
		t.Fatalf("ffx target list returned unexpected output to stderr: %s", ffxTargetListStderr)
	}

	if deviceIP == "" {
		t.Fatalf("device ip is empty")
	}

	t.Logf("Using device name: %s and device ip: %s", deviceName, deviceIP)

	// Set the default device in fconfig.
	fconfigSetDeviceArgs := []string{"set-device", deviceName, "--default"}
	t.Logf("Setting the default device by running: %s %s", tools.fconfigPath, fconfigSetDeviceArgs)
	cmd := exec.Command(tools.fconfigPath, fconfigSetDeviceArgs...)

	_, err = cmd.Output()
	if err != nil {
		t.Errorf("fconfig returned unexpected error: %s", err)
	}

	usr, err := user.Current()
	if err != nil {
		t.Fatalf("unable to get users home dir: %s", err)
	}

	expectedGetAllDeviceConfig := sdkcommon.DeviceConfig{
		DeviceName:   deviceName,
		Bucket:       "fuchsia",
		Image:        "",
		DeviceIP:     deviceIP,
		SSHPort:      "22",
		PackageRepo:  filepath.Join(usr.HomeDir, ".fuchsia", deviceName, "packages", "amber-files"),
		PackagePort:  "8083",
		IsDefault:    true,
		Discoverable: true,
	}

	// Get default device information from fconfig.
	fconfigGetAllArgs := []string{"get-all", deviceName}
	t.Logf("Getting the default device information by running: %s %s", tools.fconfigPath, fconfigGetAllArgs)
	cmd = exec.Command(tools.fconfigPath, fconfigGetAllArgs...)

	fconfigGetAllOutput, err := cmd.Output()
	var deviceConfig sdkcommon.DeviceConfig
	if err := json.Unmarshal(fconfigGetAllOutput, &deviceConfig); err != nil {
		t.Fatalf("unable to unmarshal device config from fconfig get-all: %s", err)
	}

	if diff := cmp.Diff(expectedGetAllDeviceConfig, deviceConfig); diff != "" {
		t.Errorf("fconfig get-all mismatch (-want +got):\n%s", diff)
	}

	// fssh into the default device from fconfig.
	fsshArgs := []string{"-private-key", os.Getenv(constants.SSHKeyEnvKey), "echo", "\"Hello World\""}
	t.Logf("SSH'ing into the default device by running: %s %s", tools.fsshPath, fsshArgs)
	cmd = exec.Command(tools.fsshPath, fsshArgs...)

	sshOutput, err := cmd.Output()
	if err != nil {
		t.Fatalf("fssh returned unexpected error: %s", err)
	}

	sshOutputTrimmed := strings.TrimSpace(string(sshOutput))
	expectedOutput := "Hello World"
	if sshOutputTrimmed != expectedOutput {
		t.Errorf("fssh output doesn't match; Got: %s, want: %s", sshOutputTrimmed, expectedOutput)
	}
}
