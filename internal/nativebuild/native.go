// Package nativebuild builds the pinned codec libraries used by CGO. Moon owns
// the tasks and Mise owns tool installation; this package only runs the native
// build and preserves the Windows SDK/path handling of the previous client.
package nativebuild

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
)

type Toolchain struct{ MSYSBin, CompilerBin string }

const (
	privateDirectoryMode = 0o700
	privateFileMode      = 0o600
	distributionFileMode = 0o644
)

const (
	windowsOS       = "windows"
	darwinOS        = "darwin"
	clangCommand    = "clang"
	clangCXXCommand = "clang++"
	arm64Arch       = "arm64"
	amd64Arch       = "amd64"
)

func HostToolchain(ctx context.Context) (Toolchain, error) {
	if runtime.GOOS == darwinOS {
		compiler, err := exec.CommandContext(ctx, "xcrun", "--find", clangCommand).Output()
		if err != nil {
			return Toolchain{}, fmt.Errorf("find the Xcode macOS compiler: %w", err)
		}
		return Toolchain{CompilerBin: filepath.Dir(strings.TrimSpace(string(compiler)))}, nil
	}
	if runtime.GOOS != windowsOS {
		return Toolchain{}, nil
	}
	root := os.Getenv("MSYS2_LOCATION")
	if root == "" {
		return Toolchain{}, errors.New(
			"MSYS2_LOCATION must name the CLANG64 or CLANGARM64 SDK installation",
		)
	}
	flavor := "clang64"
	if runtime.GOARCH == arm64Arch {
		flavor = "clangarm64"
	}
	return Toolchain{
		MSYSBin:     filepath.Join(root, "usr", "bin"),
		CompilerBin: filepath.Join(root, flavor, "bin"),
	}, nil
}

func (toolchain Toolchain) Command(ctx context.Context, program string, args ...string) *exec.Cmd {
	if toolchain.MSYSBin != "" {
		bin := toolchain.MSYSBin
		switch program {
		case clangCommand, clangCXXCommand, "pkg-config", "nasm":
			bin = toolchain.CompilerBin
		}
		if strings.HasPrefix(program, "llvm-") {
			bin = toolchain.CompilerBin
		}
		program = filepath.Join(bin, program+".exe")
	} else if toolchain.CompilerBin != "" && (program == clangCommand || program == clangCXXCommand) {
		program = filepath.Join(toolchain.CompilerBin, program)
	}
	// #nosec G702 -- Build executables come from the trusted local native SDK.
	command := exec.CommandContext(ctx, program, args...)
	if toolchain.MSYSBin != "" {
		// MSYS2 Bash must launch its own sed/make helpers. Git for Windows
		// utilities use another MSYS runtime and corrupt configure's quoting.
		paths := []string{toolchain.MSYSBin}
		if toolchain.CompilerBin != "" {
			paths = append([]string{toolchain.CompilerBin}, paths...)
		}
		if inherited := os.Getenv("PATH"); inherited != "" {
			paths = append(paths, inherited)
		}
		command.Env = append(
			os.Environ(),
			"PATH="+strings.Join(paths, string(os.PathListSeparator)),
		)
	}
	return command
}

func run(command *exec.Cmd) error {
	command.Stdout, command.Stderr = os.Stdout, os.Stderr
	if err := command.Run(); err != nil {
		return fmt.Errorf("%s: %w", command.Path, err)
	}
	return nil
}

func (toolchain Toolchain) Check(ctx context.Context) error {
	programs := []string{clangCommand, clangCXXCommand, "make", "pkg-config", "tar", "openssl", "bash"}
	if runtime.GOARCH == amd64Arch {
		programs = append(programs, "nasm")
	}
	for _, program := range programs {
		command := toolchain.Command(ctx, program, "--version")
		if program == "openssl" {
			command = toolchain.Command(ctx, program, "version")
		}
		if err := run(command); err != nil {
			return fmt.Errorf(
				"native build tool %s is unavailable; install the scoped Mise tools and platform SDK: %w",
				program,
				err,
			)
		}
	}
	for _, program := range []string{"cmake", "ninja", "curl"} {
		if _, err := exec.LookPath(program); err != nil {
			return err
		}
	}
	if runtime.GOOS == darwinOS {
		return run(exec.CommandContext(ctx, "xcrun", "--show-sdk-path"))
	}
	return nil
}

func (toolchain Toolchain) UnixPath(ctx context.Context, path string) (string, error) {
	if toolchain.MSYSBin == "" {
		return path, nil
	}
	raw, err := toolchain.Command(ctx, "cygpath", "-u", "--", path).Output()
	if err != nil {
		return "", fmt.Errorf("convert native path %q for MSYS2: %w", path, err)
	}
	return strings.TrimSpace(string(raw)), nil
}

func (toolchain Toolchain) Extract(ctx context.Context, archive, destination string) error {
	unix, err := toolchain.UnixPath(ctx, destination)
	if err != nil {
		return err
	}
	command := toolchain.Command(ctx, "tar", "-xf", filepath.Base(archive), "-C", unix)
	command.Dir = filepath.Dir(archive)
	return run(command)
}

func verify(path, checksum string) bool {
	file, err := os.Open(path)
	if err != nil {
		return false
	}
	defer file.Close()
	hash := sha256.New()
	if _, err = io.Copy(hash, file); err != nil {
		return false
	}
	return hex.EncodeToString(hash.Sum(nil)) == checksum
}

func archive(ctx context.Context, cache, name, url, checksum string) (string, error) {
	path := filepath.Join(cache, name)
	if verify(path, checksum) {
		return path, nil
	}
	if err := os.Remove(path); err != nil && !os.IsNotExist(err) {
		return "", err
	}
	file, err := os.CreateTemp(cache, name+".download-*")
	if err != nil {
		return "", err
	}
	temporary := file.Name()
	if err := file.Close(); err != nil {
		return "", err
	}
	defer os.Remove(temporary)
	if err := run(
		exec.CommandContext(
			ctx,
			"curl",
			"--fail",
			"--location",
			"--connect-timeout",
			"30",
			"--max-time",
			"300",
			"--output",
			temporary,
			url,
		),
	); err != nil {
		return "", err
	}
	if !verify(temporary, checksum) {
		return "", fmt.Errorf("source checksum mismatch: %s", name)
	}
	// A single Moon target owns this prefix. A verified cached download from a
	// previous interrupted build is reusable, but a partial download never is.
	if err := os.Rename(temporary, path); err != nil {
		return "", err
	}
	return path, nil
}

func Build(ctx context.Context, root string) error {
	toolchain, err := HostToolchain(ctx)
	if err != nil {
		return err
	}
	cache := filepath.Join(root, ".cache")
	prefix := filepath.Join(cache, "ffmpeg")
	sourceRoot := filepath.Join(cache, "native-src")
	if err := os.MkdirAll(prefix, privateDirectoryMode); err != nil {
		return err
	}
	if err := os.MkdirAll(sourceRoot, privateDirectoryMode); err != nil {
		return err
	}
	stamp := "go-astiav-9.0.2-openh264-2.6.0-v2-" + runtime.GOOS + "-" + runtime.GOARCH
	if runtime.GOOS == windowsOS || runtime.GOOS == darwinOS {
		if runtime.GOOS == darwinOS {
			stamp += "-apple-sdk-v1"
		} else {
			stamp += "-msys-environment-v1"
		}
		version, err := toolchain.Command(ctx, clangCommand, "--version").Output()
		if err != nil {
			return fmt.Errorf("identify native %s compiler: %w", runtime.GOOS, err)
		}
		stamp += fmt.Sprintf("-%x", sha256.Sum256(version))
	}
	if existing, err := os.ReadFile(
		filepath.Join(prefix, "build-version"),
	); err == nil &&
		strings.TrimSpace(string(existing)) == stamp {
		return nil
	}
	return buildNativeSources(ctx, toolchain, cache, prefix, sourceRoot, stamp)
}

func buildNativeSources(
	ctx context.Context,
	toolchain Toolchain,
	cache, prefix, sourceRoot, stamp string,
) error {
	ffmpeg, err := archive(
		ctx,
		cache,
		"ffmpeg-9.0.2.tar.gz",
		"https://codeload.github.com/FFmpeg/FFmpeg/tar.gz/refs/tags/n9.0.2",
		"6e374ed621e48faa40639307dff48ba6fe574a509977956d2cce9669b7cc27e9",
	)
	if err != nil {
		return err
	}
	openh264, err := archive(
		ctx,
		cache,
		"openh264-2.6.0.tar.gz",
		"https://codeload.github.com/cisco/openh264/tar.gz/refs/tags/v2.6.0",
		"558544ad358283a7ab2930d69a9ceddf913f4a51ee9bf1bfb9e377322af81a69",
	)
	if err != nil {
		return err
	}
	for _, name := range []string{"FFmpeg-n9.0.2", "openh264-2.6.0"} {
		if err := os.RemoveAll(filepath.Join(sourceRoot, name)); err != nil {
			return err
		}
	}
	if err := toolchain.Extract(ctx, ffmpeg, sourceRoot); err != nil {
		return err
	}
	if err := toolchain.Extract(ctx, openh264, sourceRoot); err != nil {
		return err
	}
	ffmpegRoot, h264Root := filepath.Join(
		sourceRoot,
		"FFmpeg-n9.0.2",
	), filepath.Join(
		sourceRoot,
		"openh264-2.6.0",
	)
	release, err := os.ReadFile(filepath.Join(ffmpegRoot, "RELEASE"))
	if err != nil {
		return err
	}
	// #nosec G703 -- The verified source archive and local build directory own this path.
	if err := os.WriteFile(
		filepath.Join(ffmpegRoot, "VERSION"),
		release,
		privateFileMode,
	); err != nil {
		return err
	}
	if err := buildNativeLibraries(ctx, toolchain, prefix, ffmpegRoot, h264Root); err != nil {
		return err
	}
	return writeNativeNotices(prefix, ffmpegRoot, h264Root, stamp)
}

func writeNativeNotices(prefix, ffmpegRoot, h264Root, stamp string) error {
	notice := []byte(
		"FFmpeg 9.0.2; source https://github.com/FFmpeg/FFmpeg/tree/n9.0.2\nBuild: intern" +
			"al/nativebuild\n",
	)
	license, err := os.ReadFile(filepath.Join(ffmpegRoot, "COPYING.LGPLv2.1"))
	if err != nil {
		return err
	}
	notice = append(notice, license...)
	// #nosec G306 G703 -- Public distribution notices inside the owned native build output.
	if err := os.WriteFile(
		filepath.Join(prefix, "COPYING.LGPLv2.1"),
		license,
		distributionFileMode,
	); err != nil {
		return err
	}
	license, err = os.ReadFile(filepath.Join(h264Root, "LICENSE"))
	if err != nil {
		return err
	}
	notice = append(
		notice,
		[]byte("\nOpenH264 2.6.0; source https://github.com/cisco/openh264/tree/v2.6.0\n")...)
	notice = append(notice, license...)
	// #nosec G306 G703 -- Public distribution notices inside the owned native build output.
	if err := os.WriteFile(
		filepath.Join(prefix, "NOTICE.txt"),
		notice,
		distributionFileMode,
	); err != nil {
		return err
	}
	// #nosec G306 G703 -- Public distribution notices inside the owned native build output.
	return os.WriteFile(
		filepath.Join(prefix, "build-version"),
		[]byte(stamp+"\n"),
		distributionFileMode,
	)
}

func buildNativeLibraries(
	ctx context.Context,
	toolchain Toolchain,
	prefix, ffmpegRoot, h264Root string,
) error {
	buildPrefix := filepath.ToSlash(prefix)
	if runtime.GOOS == windowsOS {
		raw, err := toolchain.Command(ctx, "cygpath", "-m", "--", prefix).Output()
		if err != nil {
			return err
		}
		buildPrefix = strings.TrimSpace(string(raw))
	}
	if err := buildOpenH264(ctx, toolchain, buildPrefix, h264Root); err != nil {
		return err
	}
	args := ffmpegArguments(ctx, toolchain, buildPrefix)
	command := toolchain.Command(ctx, "bash", args...)
	command.Dir = ffmpegRoot
	command.Env = append(command.Environ(), "PKG_CONFIG_PATH="+buildPrefix+"/lib/pkgconfig")
	if err := run(command); err != nil {
		return err
	}
	command = toolchain.Command(
		ctx,
		"make",
		"-j"+strconv.Itoa(runtime.NumCPU()),
		"install-libs",
		"install-headers",
	)
	command.Dir = ffmpegRoot
	if err := run(command); err != nil {
		return err
	}
	return nil
}

func buildOpenH264(ctx context.Context, toolchain Toolchain, buildPrefix, h264Root string) error {
	assembly := "No"
	arch := arm64Arch
	osName := runtime.GOOS
	cxxLibrary := "-lc++ -lm"
	if runtime.GOARCH == amd64Arch {
		assembly, arch = "Yes", "x86_64"
	}
	if runtime.GOOS == "linux" {
		cxxLibrary = "-lstdc++ -lm -lpthread"
	}
	if runtime.GOOS == windowsOS {
		osName = "mingw_nt"
	}
	jobs := "-j" + strconv.Itoa(runtime.NumCPU())
	makeArgs := []string{
		jobs,
		"PREFIX=" + buildPrefix,
		"OS=" + osName,
		"ARCH=" + arch,
		"USE_ASM=" + assembly,
		"CC=" + filepath.ToSlash(toolchain.Command(ctx, clangCommand).Path),
		"CXX=" + filepath.ToSlash(toolchain.Command(ctx, clangCXXCommand).Path),
		"STATIC_LDFLAGS=" + cxxLibrary,
		"BUILDTYPE=Release",
		"install-static",
	}
	if ar := os.Getenv("AR"); ar != "" {
		makeArgs = append(makeArgs, "AR="+ar)
	}
	makeArgs = append(makeArgs, "ASM="+filepath.ToSlash(toolchain.Command(ctx, "nasm").Path))
	if runtime.GOOS == windowsOS {
		makeArgs = append(makeArgs, "AR="+filepath.ToSlash(toolchain.Command(ctx, "llvm-ar").Path))
	}
	command := toolchain.Command(ctx, "make", makeArgs...)
	command.Dir = h264Root
	if err := run(command); err != nil {
		return err
	}
	return nil
}

func ffmpegArguments(ctx context.Context, toolchain Toolchain, buildPrefix string) []string {
	args := []string{
		"./configure",
		"--prefix=" + buildPrefix,
		"--cc=" + filepath.ToSlash(toolchain.Command(ctx, clangCommand).Path),
		"--cxx=" + filepath.ToSlash(toolchain.Command(ctx, clangCXXCommand).Path),
		"--disable-everything",
		"--disable-autodetect",
		"--disable-programs",
		"--disable-doc",
		"--disable-debug",
		"--disable-network",
		"--disable-shared",
		"--enable-static",
		"--enable-pic",
		"--enable-avdevice",
		"--enable-avfilter",
		"--enable-swscale",
		"--enable-swresample",
		"--enable-libopenh264",
		"--enable-protocol=file",
		"--enable-demuxer=mov,matroska,ogg,aac",
		"--enable-muxer=ipod,mp4,hls,mpegts",
		"--enable-decoder=aac,opus,vorbis,h264,vp9,hevc",
		"--enable-encoder=aac,libopenh264",
		"--enable-parser=aac,opus,vorbis,h264,vp9,hevc",
		"--enable-bsf=aac_adtstoasc,h264_mp4toannexb",
		"--enable-filter=abuffer,abuffersink,aresample,aformat,anull,buffer,buffersink,sc" +
			"ale,format,fps,null",
	}
	if runtime.GOARCH == arm64Arch {
		args = append(args, "--arch=aarch64", "--disable-x86asm")
	}
	if runtime.GOOS == windowsOS {
		args = append(
			args,
			"--target-os=mingw32",
			"--pkg-config="+filepath.ToSlash(toolchain.Command(ctx, "pkg-config").Path),
			"--extra-ldflags=-static",
		)
		for _, program := range []string{"llvm-ar", "llvm-ranlib", "llvm-nm"} {
			args = append(
				args,
				"--"+strings.TrimPrefix(
					program,
					"llvm-",
				)+"="+filepath.ToSlash(
					toolchain.Command(ctx, program).Path,
				),
			)
		}
	}
	if runtime.GOARCH == amd64Arch {
		args = append(args, "--x86asmexe="+filepath.ToSlash(toolchain.Command(ctx, "nasm").Path))
	}
	return args
}
