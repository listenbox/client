package nativebuild

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func TestMsysTarExtractsIntoWindowsPathsWithSpaces(t *testing.T) {
	t.Parallel()
	if runtime.GOOS != windowsOS {
		t.Skip("MSYS2 native Windows path regression")
	}
	toolchain, err := HostToolchain()
	if err != nil {
		t.Fatal(err)
	}
	root := filepath.Join(t.TempDir(), "ffmpeg archive paths")
	source, destination := filepath.Join(
		root,
		"source files",
	), filepath.Join(
		root,
		"extracted files",
	)
	for _, path := range []string{source, destination} {
		if err := os.MkdirAll(path, 0o700); err != nil {
			t.Fatal(err)
		}
	}
	const contents = "archive path regression"
	if err := os.WriteFile(
		filepath.Join(source, "fixture.txt"),
		[]byte(contents),
		0o600,
	); err != nil {
		t.Fatal(err)
	}
	command := toolchain.Command(
		t.Context(),
		"tar",
		"-cf",
		"fixture.tar",
		"-C",
		"source files",
		"fixture.txt",
	)
	command.Dir = root
	if err := run(command); err != nil {
		t.Fatal(err)
	}
	if err := toolchain.Extract(
		t.Context(),
		filepath.Join(root, "fixture.tar"),
		destination,
	); err != nil {
		t.Fatal(err)
	}
	actual, err := os.ReadFile(filepath.Join(destination, "fixture.txt"))
	if err != nil || string(actual) != contents {
		t.Fatalf("extracted archive: %q %v", actual, err)
	}
}

func TestMsysToolsIgnoreAmbientSearchPaths(t *testing.T) {
	t.Parallel()
	root := filepath.Join(t.TempDir(), "ffmpeg tool paths")
	bin, unrelated := filepath.Join(root, "msys64", "usr", "bin"), filepath.Join(root, "unrelated")
	for _, path := range []string{bin, unrelated} {
		if err := os.MkdirAll(path, 0o700); err != nil {
			t.Fatal(err)
		}
	}
	binary := sdkFixtureExecutable(t)
	toolchain := Toolchain{MSYSBin: bin}
	for _, program := range []string{"bash", "make", "tar", "openssl"} {
		executable := filepath.Join(bin, program+".exe")
		// #nosec G306 G703 -- The test owns this executable SDK fixture.
		if err := os.WriteFile(executable, binary, 0o700); err != nil {
			t.Fatal(err)
		}
		command := toolchain.Command(
			t.Context(),
			program,
			"-test.list=TestMsysToolsIgnoreAmbientSearchPaths",
		)
		command.Env = append(os.Environ(), "PATH="+unrelated)
		command.Dir = unrelated
		output, err := command.CombinedOutput()
		if err != nil ||
			!strings.Contains(string(output), "TestMsysToolsIgnoreAmbientSearchPaths") {
			t.Fatalf("launch SDK %s with an empty search path: %v %s", program, err, output)
		}
		name := program
		if runtime.GOOS == windowsOS {
			name += ".exe"
		}
		if err := os.Rename(executable, filepath.Join(unrelated, name)); err != nil {
			t.Fatal(err)
		}
		command = toolchain.Command(t.Context(), program, "--version")
		command.Env = append(os.Environ(), "PATH="+unrelated)
		if err := command.Run(); err == nil || !os.IsNotExist(err) {
			t.Fatalf("missing SDK %s fell back to an ambient executable: %v", program, err)
		}
	}
}

func sdkFixtureExecutable(t *testing.T) []byte {
	t.Helper()
	self, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	binary, err := os.ReadFile(self)
	if err != nil {
		t.Fatal(err)
	}
	return binary
}
