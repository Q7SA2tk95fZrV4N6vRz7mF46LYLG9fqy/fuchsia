// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package emulator

import (
	"archive/tar"
	"bufio"
	"bytes"
	"compress/gzip"
	"fmt"
	"io"
	"io/ioutil"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"go.fuchsia.dev/fuchsia/tools/build"
	"go.fuchsia.dev/fuchsia/tools/lib/jsonutil"
	"go.fuchsia.dev/fuchsia/tools/qemu"
	"go.fuchsia.dev/fuchsia/tools/virtual_device"
	fvdpb "go.fuchsia.dev/fuchsia/tools/virtual_device/proto"
)

// Untar untars a tar.gz file into a directory.
func untar(dst string, src string) error {
	f, err := os.Open(src)
	if err != nil {
		return err
	}
	defer f.Close()

	gz, err := gzip.NewReader(f)
	if err != nil {
		return err
	}
	defer gz.Close()
	for tr := tar.NewReader(gz); ; {
		header, err := tr.Next()
		if err == io.EOF {
			return nil
		} else if err != nil {
			return err
		}

		path := filepath.Join(dst, header.Name)
		info := header.FileInfo()
		if info.IsDir() {
			if err := os.MkdirAll(path, info.Mode()); err != nil {
				return err
			}
		} else {
			if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
				return err
			}

			f, err := os.OpenFile(path, os.O_CREATE|os.O_EXCL|os.O_WRONLY, info.Mode())
			if err != nil {
				return err
			}
			_, err = io.Copy(f, tr)
			f.Close()
			if err != nil {
				return err
			}
		}
	}
}

// Distribution is a collection of QEMU-related artifacts.
//
// Delete must be called once done with it.
type Distribution struct {
	testDataDir  string
	unpackedPath string
	Emulator     Emulator
}

// Arch is the architecture to emulate.
type Arch string

const (
	X64   Arch = "x64"
	Arm64 Arch = "arm64"
)

// Emulator is the emulator to use.
type Emulator int

const (
	Qemu Emulator = iota
	Femu
)

// Disk represents a single disk that will be attached to the virtual machine.
type Disk struct {
	Path string
	USB  bool
}

// Instance is a live emulator instance.
type Instance struct {
	cmd      *exec.Cmd
	piped    *exec.Cmd
	stdin    *bufio.Writer
	stdout   *bufio.Reader
	stderr   *bufio.Reader
	emulator Emulator
}

// DistributionParams is passed to UnpackFrom().
type DistributionParams struct {
	Emulator Emulator
}

// buildInfo carries information about the Fuchsia build.
//
// This is read from a file generated by BUILD.gn.
type buildInfo struct {
	// The path to the image.json manifest produced by the build.
	//
	// This path is relative to the file from which this struct was read.
	ImageManifestPath string

	// The build's target CPU architecture.
	TargetCPU string
}

// DefaultVirtualDevice returns a virtual device configuration for testing.
//
// The returned virtual device is compatible with the image manifest produced by a Fuchsia
// build for the specified architecture.
func DefaultVirtualDevice(arch string) *fvdpb.VirtualDevice {
	return &fvdpb.VirtualDevice{
		Name:   "default",
		Kernel: "qemu-kernel",
		Initrd: "zircon-a",
		Hw: &fvdpb.HardwareProfile{
			Arch:      arch,
			Mac:       "52:54:00:63:5e:7a",
			Ram:       "8K",
			CpuCount:  8,
			EnableKvm: arch == "x64",
		},
		KernelArgs: []string{
			"kernel.serial=legacy",
			"kernel.entropy-mixin=1420bb81dc0396b37cc2d0aa31bb2785dadaf9473d0780ecee1751afb5867564",
			"kernel.halt-on-panic=true",
			// Disable lockup detector heartbeats. In emulated environments, virtualized CPUs
			// may be starved or fail to execute in a timely fashion, resulting in apparent
			// lockups. See fxbug.dev/65990.
			"kernel.lockup-detector.heartbeat-period-ms=0",
			"kernel.lockup-detector.heartbeat-age-threshold-ms=0",
		},
	}
}

// UnpackFrom unpacks the emulator distribution.
//
// path is the path to host_x64/test_data containing the emulator.
// emulator is the emulator to unpack.
func UnpackFrom(path string, distroParams DistributionParams) (*Distribution, error) {
	// Since the emulator will be started from a different directory, make the base path
	// absolute.
	path, err := filepath.Abs(path)
	if err != nil {
		return nil, err
	}
	emulator_file := "qemu.tar.gz"
	if distroParams.Emulator == Femu {
		emulator_file = "femu.tar.gz"
	}
	archivePath := filepath.Join(path, "emulator", emulator_file)
	unpackedPath, err := ioutil.TempDir("", "emulator-distro")
	if err != nil {
		return nil, err
	}
	if err = untar(unpackedPath, archivePath); err != nil {
		os.RemoveAll(unpackedPath)
		return nil, err
	}
	return &Distribution{testDataDir: path, unpackedPath: unpackedPath, Emulator: distroParams.Emulator}, nil
}

// Delete removes the emulator-related artifacts.
func (d *Distribution) Delete() error {
	return os.RemoveAll(d.unpackedPath)
}

func (d *Distribution) systemPath(arch Arch) string {
	if d.Emulator == Femu {
		// FEMU has one binary for all arches.
		return filepath.Join(d.unpackedPath, "emulator")
	}
	switch arch {
	case X64:
		return filepath.Join(d.unpackedPath, "bin", "qemu-system-x86_64")
	case Arm64:
		return filepath.Join(d.unpackedPath, "bin", "qemu-system-aarch64")
	}
	return ""
}

// TargetCPU returs the target CPU used by the build that produced this library.
func (d *Distribution) TargetCPU() (Arch, error) {
	buildinfo, err := d.loadBuildInfo()
	if err != nil {
		return X64, err
	}
	switch buildinfo.TargetCPU {
	case "x64":
		return X64, nil
	case "arm64":
		return Arm64, nil
	}
	return X64, fmt.Errorf("unknown target CPU: %s", buildinfo.TargetCPU)
}

func (d *Distribution) buildCommandLine(fvd *fvdpb.VirtualDevice, images build.ImageManifest) ([]string, error) {
	if d.Emulator == Femu {
		b := qemu.NewAEMUCommandBuilder()
		b.SetBinary(d.systemPath(Arch(fvd.Hw.Arch)))
		// Ask QEMU to emit a message on stderr once the VM is running
		// so we'll know whether QEMU has started or not.
		b.SetFlag("-trace", "enable=vm_state_notify")
		b.SetFlag("-nographic")
		if err := virtual_device.AEMUCommand(b, fvd, images); err != nil {
			return nil, err
		}
		return b.Build()
	}

	b := &qemu.QEMUCommandBuilder{}
	b.SetBinary(d.systemPath(Arch(fvd.Hw.Arch)))
	// Ask QEMU to emit a message on stderr once the VM is running
	// so we'll know whether QEMU has started or not.
	b.SetFlag("-trace", "enable=vm_state_notify")
	b.SetFlag("-nographic")
	if err := virtual_device.QEMUCommand(b, fvd, images); err != nil {
		return nil, err
	}
	return b.Build()

}

// Create creates an instance of the emulator with the given parameters.
func (d *Distribution) Create(fvd *fvdpb.VirtualDevice) (*Instance, error) {
	images, err := d.loadImageManifest()
	if err != nil {
		return nil, err
	}

	args, err := d.buildCommandLine(fvd, images)
	if err != nil {
		return nil, err
	}

	fmt.Printf("Running %s %s\n", args[0], args[1:])

	i := &Instance{
		cmd:      exec.Command(args[0], args[1:]...),
		emulator: d.Emulator,
	}
	// QEMU looks in the cwd for some specially named files, in particular
	// multiboot.bin, so avoid picking those up accidentally. See
	// https://fxbug.dev/53751.
	// TODO(fxbug.dev/58804): Remove this.
	i.cmd.Dir = "/"

	return i, nil
}

// RunNonInteractive runs an instance of the emulator that runs a single command and
// returns the log that results from doing so.
//
// This mode is non-interactive and is intended specifically to test the case
// where the serial port has been disabled. The following modifications are
// made to the emulator invocation compared with Create()/Start():
//
//  - amalgamate the given ZBI into a larger one that includes an additional
//    entry of a script which includes commands to run.
//  - that script mounts a disk created on the host in /tmp, and runs the
//    given command with output redirected to a file also on the /tmp disk
//  - the script triggers shutdown of the machine
//  - after emulator shutdown, the log file is extracted and returned.
//
// In order to achieve this, here we need to create the host minfs
// file system, write the commands to run, build the augmented .zbi to
// be used to boot. We then use Start() and wait for shutdown.
// Finally, extract and return the log from the minfs disk.
func (d *Distribution) RunNonInteractive(toRun, hostPathMinfsBinary, hostPathZbiBinary string, fvd *fvdpb.VirtualDevice) (string, string, error) {
	root, err := ioutil.TempDir("", "qemu")
	if err != nil {
		return "", "", err
	}
	log, logerr, err := d.runNonInteractive(root, toRun, hostPathMinfsBinary, hostPathZbiBinary, fvd)
	if err2 := os.RemoveAll(root); err == nil {
		err = err2
	}
	return log, logerr, err
}

func (d *Distribution) runNonInteractive(root, toRun, hostPathMinfsBinary, hostPathZbiBinary string, fvd *fvdpb.VirtualDevice) (string, string, error) {
	// Write runcmds that mounts the results disk, runs the requested command, and
	// shuts down.
	script := `mkdir /tmp/testdata-fs
waitfor class=block topo=/dev/sys/pci/00:06.0/virtio-block/block timeout=60000
mount /dev/sys/pci/00:06.0/virtio-block/block /tmp/testdata-fs
` + toRun + ` 2>/tmp/testdata-fs/err.txt >/tmp/testdata-fs/log.txt
umount /tmp/testdata-fs
dm poweroff
`
	runcmds := filepath.Join(root, "runcmds.txt")
	if err := ioutil.WriteFile(runcmds, []byte(script), 0666); err != nil {
		return "", "", err
	}
	// Make a minfs filesystem to mount in the target.
	fs := filepath.Join(root, "a.fs")
	cmd := exec.Command(hostPathMinfsBinary, fs+"@100M", "mkfs")
	if err := cmd.Run(); err != nil {
		return "", "", err
	}

	images, err := d.loadImageManifest()
	if err != nil {
		return "", "", err
	}

	oldZBIPath := ""
	newZBIPath := filepath.Join(root, "a.zbi")

	// Replace the ZBI in the image manifest with our modified one.
	for i, image := range images {
		if image.Name == fvd.Initrd && image.Type == "zbi" {
			oldZBIPath = image.Path
			images[i].Path = newZBIPath
			break
		}
	}

	// Create the new initrd that references the runcmds file.
	cmd = exec.Command(hostPathZbiBinary, "-o", newZBIPath, oldZBIPath, "-e", "runcmds="+runcmds)
	if err := cmd.Run(); err != nil {
		return "", "", err
	}

	fvd.KernelArgs = append(fvd.KernelArgs, "zircon.autorun.boot=/boot/bin/sh+/boot/runcmds")

	// Add the temporary disk at 00:06.0. This follows how infra runs qemu with an extra
	// disk via botanist.
	fvd.Drive = &fvdpb.Drive{
		Id:         "resultdisk",
		Image:      fs,
		IsFilename: true,
		PciAddress: "6.0",
	}

	args, err := d.buildCommandLine(fvd, images)
	if err != nil {
		return "", "", err
	}

	fmt.Printf("Running non-interactive %s %s\n", args[0], args[1:])

	cmd = exec.Command(args[0], args[1:]...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	// QEMU looks in the cwd for some specially named files, in particular
	// multiboot.bin, so avoid picking those up accidentally. See
	// https://fxbug.dev/53751.
	// TODO(fxbug.dev/58804): Remove this.
	cmd.Dir = "/"
	if err := cmd.Start(); err != nil {
		return "", "", err
	}
	defer cmd.Process.Kill()
	if err := cmd.Wait(); err != nil {
		return "", "", err
	}

	log := filepath.Join(root, "log.txt")
	logerr := filepath.Join(root, "err.txt")
	cmd = exec.Command(hostPathMinfsBinary, fs, "cp", "::/log.txt", log)
	if err := cmd.Run(); err != nil {
		return "", "", err
	}
	cmd = exec.Command(hostPathMinfsBinary, fs, "cp", "::/err.txt", logerr)
	if err := cmd.Run(); err != nil {
		return "", "", err
	}

	retLog, err := ioutil.ReadFile(log)
	if err != nil {
		return "", "", err
	}
	retErr, err := ioutil.ReadFile(logerr)
	if err != nil {
		return "", "", err
	}
	fmt.Printf("===== %s non-interactive run stdout =====\n%s\n", toRun, retLog)
	fmt.Printf("===== %s non-interactive run stderr =====\n%s\n", toRun, retErr)
	fmt.Printf("===== %s end =====\n", toRun)
	return string(retLog), string(retErr), nil
}

// Decodes the buildinfo.ini file generated by BUILD.gn
func (d *Distribution) loadBuildInfo() (*buildInfo, error) {
	var buildinfo buildInfo

	path := filepath.Join(d.testDataDir, "emulator", "buildinfo.ini")
	data, err := ioutil.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("read %q: %w", path, err)
	}

	scanner := bufio.NewScanner(bytes.NewReader(data))
	for scanner.Scan() {
		line := scanner.Text()
		if strings.TrimSpace(line) == "" {
			continue
		}

		kv := strings.SplitN(line, "=", 2)
		if len(kv) != 2 {
			return nil, fmt.Errorf("invalid buildinfo line: %q", line)
		}
		switch kv[0] {
		case "image_manifest_path":
			buildinfo.ImageManifestPath = kv[1]
		case "target_cpu":
			buildinfo.TargetCPU = kv[1]
		default:
			return nil, fmt.Errorf("unknown buildinfo key: %q", kv[0])
		}
	}

	return &buildinfo, nil
}

func (d *Distribution) loadImageManifest() (build.ImageManifest, error) {
	buildinfo, err := d.loadBuildInfo()
	if err != nil {
		return nil, err
	}

	var images build.ImageManifest

	// ImageManifestPath is given as a relative path from test_data/emulator.
	imageManifestPath := filepath.Clean(filepath.Join(d.testDataDir, "emulator", buildinfo.ImageManifestPath))
	if err := jsonutil.ReadFromFile(imageManifestPath, &images); err != nil {
		return nil, fmt.Errorf("json read %q: %w", imageManifestPath, err)
	}

	// Patch each image in the manifest to have an absolute path. The paths are already
	// relative to the image manifest.
	imageManifestDir := filepath.Dir(imageManifestPath)
	for i := range images {
		if !filepath.IsAbs(images[i].Path) {
			images[i].Path = filepath.Join(imageManifestDir, images[i].Path)
		}
	}

	return images, nil
}

// Start the emulator instance.
func (i *Instance) Start() error {
	return i.StartPiped(nil)
}

// StartPiped starts the emulator instance with stdin/stdout piped through a
// different process.
//
// Assumes that the stderr from the piped process should replace the stdout
// from the emulator.
func (i *Instance) StartPiped(piped *exec.Cmd) error {
	stdin, err := i.cmd.StdinPipe()
	if err != nil {
		return err
	}
	stdout, err := i.cmd.StdoutPipe()
	if err != nil {
		return err
	}
	stderr, err := i.cmd.StderrPipe()
	if err != nil {
		return err
	}

	if piped != nil {
		piped.Stdin = stdout
		piped.Stdout = stdin
		pipedStderr, err := piped.StderrPipe()
		if err != nil {
			return err
		}
		i.stdout = bufio.NewReader(pipedStderr)
		i.stderr = bufio.NewReader(stderr)
		if err = piped.Start(); err != nil {
			return err
		}
		i.piped = piped
	} else {
		i.stdin = bufio.NewWriter(stdin)
		i.stdout = bufio.NewReader(stdout)
		i.stderr = bufio.NewReader(stderr)
	}

	startErr := i.cmd.Start()

	// Look for very early log message to validate that the emulator likely started
	// correctly. Loop for a while to give the emulator a chance to boot.
	fmt.Println("Checking for QEMU boot...")
	for j := 0; j < 100; j++ {

		if i.emulator == Femu {
			// FEMU isn't built with support for outputting trace events.
			// Instead we look for a message that occurs very early during Zircon boot.
			if i.checkForLogMessage(i.stdout, "welcome to Zircon") == nil {
				break
			}
		} else {
			// The flag `-trace enable=vm_state_notify` will cause qemu to
			// print this message early in boot.
			if i.checkForLogMessage(i.stderr, "vm_state_notify running") == nil {
				break
			}
		}

		time.Sleep(100 * time.Millisecond)
	}

	return startErr
}

// Kill terminates the emulator instance.
func (i *Instance) Kill() error {
	var err error
	if i.piped != nil {
		err = i.piped.Process.Kill()
	}
	if err2 := i.cmd.Process.Kill(); err2 != nil {
		return err2
	}
	return err
}

func printWhileWait(r *bufio.Reader, proc *os.Process) (*os.ProcessState, error) {
	stop := make(chan struct{})
	defer close(stop)
	go func() {
		for {
			select {
			case <-stop:
				return
			default:
				if line, err := r.ReadString('\n'); err == nil {
					fmt.Print(line)
				}
			}
		}
	}()

	ps, err := proc.Wait()
	return ps, err
}

// Wait for the emulator instance to terminate
func (i *Instance) Wait() (*os.ProcessState, error) {
	if i.piped != nil {
		if ps, err := printWhileWait(i.stdout, i.piped.Process); err != nil {
			return ps, err
		}
	}
	return printWhileWait(i.stdout, i.cmd.Process)
}

// RunCommand runs the given command in the serial console for the emulator
// instance.
func (i *Instance) RunCommand(cmd string) error {
	if _, err := fmt.Fprintf(i.stdin, "%s\n", cmd); err != nil {
		return err
	}
	return i.stdin.Flush()
}

// WaitForLogMessage reads log messages from the emulator instance until it reads a
// message that contains the given string.
func (i *Instance) WaitForLogMessage(msg string) error {
	return i.WaitForLogMessages([]string{msg})
}

// WaitForLogMessages reads log messages from the emulator instance until it reads all
// message in |msgs|. The log messages can appear in *any* order. Only one
// expected message from |msgs| is retired per matching actual message even if
// multiple messages from |msgs| match the log line.
func (i *Instance) WaitForLogMessages(msgs []string) error {
	return i.checkForLogMessages(i.stdout, msgs)
}

// WaitForAnyLogMessage reads log messages from the emulator instance looking for any line that contains a message from msgs.
// Returns the first message that was found, or an error.
func (i *Instance) WaitForAnyLogMessage(msgs ...string) (string, error) {
	for {
		line, err := i.stdout.ReadString('\n')
		if err != nil {
			i.printStderr()
			return "", err
		}
		for _, msg := range msgs {
			if strings.Contains(line, msg) {
				return msg, nil
			}
		}
	}
}

// WaitForLogMessageAssertNotSeen is the same as WaitForLogMessage() but with
// the addition that it will return an error if |notSeen| is contained in a
// retrieved message.
func (i *Instance) WaitForLogMessageAssertNotSeen(msg string, notSeen string) error {
	for {
		line, err := i.stdout.ReadString('\n')
		if err != nil {
			return fmt.Errorf("failed to find: %q: %w", msg, err)
		}
		fmt.Print(line)
		if strings.Contains(line, msg) {
			return nil
		}
		if strings.Contains(line, notSeen) {
			return fmt.Errorf("found in output: %q", msg)
		}
	}
}

// AssertLogMessageNotSeenWithinTimeout will fail if |notSeen| is seen within the
// |timeout| period. This function will timeout as success if more than |timeout| has
// passed without seeing |notSeen|.
func (i *Instance) AssertLogMessageNotSeenWithinTimeout(notSeen string, timeout time.Duration) error {
	// ReadString is blocking, we need to make sure it respects the global timeout.
	seen := make(chan struct{})
	stop := make(chan struct{})
	defer close(stop)
	go func() {
		defer close(seen)
		for {
			select {
			case <-stop:
				return
			default:
				if line, err := i.stdout.ReadString('\n'); err == nil {
					if strings.Contains(line, notSeen) {
						seen <- struct{}{}
						return
					}
				}
			}
		}
	}()
	select {
	case <-seen:
		return fmt.Errorf("found in output: %q", notSeen)
	case <-time.After(timeout):
		return nil
	}
}

// Reset display: ESC c
// Reset screen mode: ESC [ ? 7 l
// Move cursor home: ESC [ 2 J
// All text attributes off: ESC [ 0 m
const emuClearPrefix = "\x1b\x63\x1b\x5b\x3f\x37\x6c\x1b\x5b\x32\x4a\x1b\x5b\x30\x6d"

// Reads all messages from |b| and tests if |msg| appears. Returns error if any.
func (i *Instance) checkForLogMessage(b *bufio.Reader, msg string) error {
	return i.checkForLogMessages(b, []string{msg})
}

// printStderr prints all the lines from the instance's stderr stream.
func (i *Instance) printStderr() {
	for {
		stderr, err := i.stderr.ReadString('\n')
		if err != nil {
			return
		}
		fmt.Print(stderr)
	}
}

// Reads all messages from |b| and tests if all messages of |msgs| appear in *any* order. Returns
// error if any.
func (i *Instance) checkForLogMessages(b *bufio.Reader, msgs []string) error {
	for {
		line, err := b.ReadString('\n')
		if err != nil {
			i.printStderr()
			return err
		}

		// Drop the clearing preamble as it makes it difficult to see output
		// when there's multiple emulator runs in a single binary.
		toPrint := line
		if strings.HasPrefix(toPrint, emuClearPrefix) {
			toPrint = toPrint[len(emuClearPrefix):]
		}
		fmt.Print(toPrint)
		for i, msg := range msgs {
			if strings.Contains(line, msg) {
				msgs = append(msgs[:i], msgs[i+1:]...)
				if len(msgs) == 0 {
					return nil
				}
				break
			}
		}
	}
}
