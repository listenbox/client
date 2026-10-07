//! Pinned native dependency preparation, shared by every client target.
use std::{
    env,
    error::Error,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn unix_command(program: &str, msys2_bin: Option<&Path>) -> Command {
    match msys2_bin {
        // Windows searches its system directory before PATH for bare program
        // names, which can launch WSL's bash.exe instead of MSYS2's Bash.
        Some(bin) => Command::new(bin.join(format!("{program}.exe"))),
        None => Command::new(program),
    }
}

fn run(command: &mut Command) -> Result<(), Box<dyn Error>> {
    let status = command.status()?;
    if !status.success() {
        return Err(format!("{command:?} failed: {status}").into());
    }
    Ok(())
}

fn unix_path(path: &Path, msys2_bin: Option<&Path>) -> Result<OsString, Box<dyn Error>> {
    let Some(bin) = msys2_bin else {
        return Ok(path.as_os_str().to_owned());
    };
    // Native Windows processes do not get MSYS2's automatic argument conversion.
    // In particular, GNU tar cannot use a backslash-separated Windows -C path.
    let output = unix_command("cygpath", Some(bin))
        .args(["-u", "--"])
        .arg(path)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cygpath failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?.trim_end().into())
}

fn checksum_matches(
    archive: &Path,
    checksum: &str,
    msys2_bin: Option<&Path>,
) -> Result<bool, Box<dyn Error>> {
    let digest = unix_command("openssl", msys2_bin)
        .args(["dgst", "-sha256"])
        .arg(unix_path(archive, msys2_bin)?)
        .output()?;
    if !digest.status.success() {
        return Err(format!(
            "cannot checksum {}: {}",
            archive.display(),
            String::from_utf8_lossy(&digest.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(digest.stdout)?.trim().ends_with(checksum))
}

fn prepare_archive(
    archive: &Path,
    url: &str,
    checksum: &str,
    msys2_bin: Option<&Path>,
) -> Result<(), Box<dyn Error>> {
    if archive.exists() {
        if checksum_matches(archive, checksum, msys2_bin)? {
            return Ok(());
        }
        eprintln!("Discarding corrupt cached archive: {}", archive.display());
        fs::remove_file(archive)?;
    }
    // Interrupted downloads must never become reusable source archives. Give
    // concurrent target builds separate temporary files in the same directory.
    let download = archive.with_extension(format!("download-{}", std::process::id()));
    let result = (|| -> Result<(), Box<dyn Error>> {
        run(Command::new("curl")
            .args([
                "--fail",
                "--location",
                "--connect-timeout",
                "30",
                "--max-time",
                "300",
                "--output",
            ])
            .arg(&download)
            .arg(url))?;
        if !checksum_matches(&download, checksum, msys2_bin)? {
            return Err(format!("{} source checksum mismatch", archive.display()).into());
        }
        // Another target may have published the same verified archive while
        // this download was running. Windows cannot rename over that file.
        if archive.exists() && checksum_matches(archive, checksum, msys2_bin)? {
            return Ok(());
        }
        fs::rename(&download, archive)?;
        Ok(())
    })();
    if download.exists() {
        fs::remove_file(&download)?;
    }
    result
}

fn extract_archive(
    archive: &Path,
    destination: &Path,
    msys2_bin: Option<&Path>,
) -> Result<(), Box<dyn Error>> {
    run(unix_command("tar", msys2_bin)
        .current_dir(archive.parent().ok_or("missing archive directory")?)
        .arg("-xf")
        .arg(archive.file_name().ok_or("missing archive filename")?)
        .arg("-C")
        .arg(unix_path(destination, msys2_bin)?))
}

// Pin the assembler with the native libraries; an ambient NASM installation
// must not decide whether shipped x86 code uses accelerated codec/scaling paths.
fn prepare_nasm(cache: &Path, prefix: &Path) -> Result<Option<OsString>, Box<dyn Error>> {
    if !cfg!(all(unix, target_arch = "x86_64")) {
        return Ok(None);
    }
    let version = "2.16.03";
    let checksum = "1412a1c760bbd05db026b6c0d1657affd6631cd0a63cddb6f73cc6d4aa616148";
    let archive = cache.join(format!("nasm-{version}.tar.xz"));
    prepare_archive(
        &archive,
        &format!("https://www.nasm.us/pub/nasm/releasebuilds/{version}/nasm-{version}.tar.xz"),
        checksum,
        None,
    )?;
    let source = cache.join(format!("nasm-{version}"));
    if source.exists() {
        fs::remove_dir_all(&source)?;
    }
    extract_archive(&archive, cache, None)?;
    run(Command::new("bash").arg("./configure").current_dir(&source))?;
    run(Command::new("make")
        .arg(format!("-j{}", std::thread::available_parallelism()?))
        .arg("nasm")
        .current_dir(&source))?;
    fs::create_dir_all(prefix.join("bin"))?;
    fs::copy(source.join("nasm"), prefix.join("bin/nasm"))?;
    fs::copy(source.join("LICENSE"), prefix.join("NASM-LICENSE.txt"))?;
    let paths = std::iter::once(prefix.join("bin")).chain(
        env::split_paths(&env::var_os("PATH").ok_or("PATH is required")?).collect::<Vec<_>>(),
    );
    Ok(Some(env::join_paths(paths)?))
}

fn main() -> Result<(), Box<dyn Error>> {
    let target = env::args().nth(1);
    let arch = match target.as_deref() {
        Some("x86_64-pc-windows-msvc") => Some("x86_64"),
        Some("aarch64-pc-windows-msvc") => Some("aarch64"),
        Some(_) => return Err("unsupported FFmpeg target".into()),
        None => None,
    };
    if target.is_some() && !cfg!(windows) {
        return Err(
            "the Windows FFmpeg build requires a Windows MSVC developer environment".into(),
        );
    }
    let msys2_bin = if target.is_some() {
        Some(
            PathBuf::from(env::var_os("MSYS2_LOCATION").ok_or("MSYS2_LOCATION is required")?)
                .join("usr/bin"),
        )
    } else {
        None
    };
    let version = "9.0.2";
    let checksum = "8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e";
    let cache = env::current_dir()?.join(".cache");
    let prefix = cache.join(match &target {
        Some(target) => format!("ffmpeg-{target}"),
        None => "ffmpeg".into(),
    });
    let build = match &target {
        Some(target) => cache.join(format!("ffmpeg-build-{target}")),
        None => cache.clone(),
    };
    fs::create_dir_all(&cache)?;
    fs::create_dir_all(&build)?;
    let archive = cache.join(format!("ffmpeg-{version}.tar.xz"));
    let url = format!("https://ffmpeg.org/releases/ffmpeg-{version}.tar.xz");
    prepare_archive(&archive, &url, checksum, msys2_bin.as_deref())?;
    let source = build.join(format!("ffmpeg-{version}"));
    if source.exists() {
        fs::remove_dir_all(&source)?;
    }
    extract_archive(&archive, &build, msys2_bin.as_deref())?;
    let openh264_version = "2.6.0";
    let openh264_checksum = "558544ad358283a7ab2930d69a9ceddf913f4a51ee9bf1bfb9e377322af81a69";
    let openh264_url =
        format!("https://codeload.github.com/cisco/openh264/tar.gz/refs/tags/v{openh264_version}");
    let openh264_archive = cache.join(format!("openh264-{openh264_version}.tar.gz"));
    prepare_archive(
        &openh264_archive,
        &openh264_url,
        openh264_checksum,
        msys2_bin.as_deref(),
    )?;
    let openh264_source = build.join(format!("openh264-{openh264_version}"));
    if openh264_source.exists() {
        fs::remove_dir_all(&openh264_source)?;
    }
    extract_archive(&openh264_archive, &build, msys2_bin.as_deref())?;
    let native_path = prepare_nasm(&cache, &prefix)?;
    let mut make = unix_command("make", msys2_bin.as_deref());
    make.current_dir(&openh264_source)
        .arg(format!("-j{}", std::thread::available_parallelism()?))
        .arg(format!(
            "PREFIX={}",
            Path::new(&unix_path(&prefix, msys2_bin.as_deref())?).display()
        ))
        .arg(if cfg!(windows) {
            "USE_ASM=No"
        } else {
            "USE_ASM=Yes"
        })
        .args(["BUILDTYPE=Release", "install-static"]);
    if let Some(path) = &native_path {
        make.env("PATH", path);
    }
    if let Some(arch) = arch {
        make.args(["OS=msvc", &format!("ARCH={arch}")]);
    }
    run(&mut make)?;
    let mut configure = unix_command("bash", msys2_bin.as_deref());
    configure
        .arg("./configure")
        .current_dir(&source)
        .env("PKG_CONFIG_PATH", prefix.join("lib/pkgconfig"));
    if let Some(path) = &native_path {
        configure.env("PATH", path);
    }
    if native_path.is_none() {
        configure.arg("--disable-x86asm");
    }
    if let Some(arch) = arch {
        configure
            .args(["--toolchain=msvc", "--extra-cflags=-MT"])
            .arg(format!("--arch={arch}"));
        if arch == "aarch64" {
            configure.arg("--disable-asm");
        }
    }
    let unix_prefix = unix_path(&prefix, msys2_bin.as_deref())?;
    run(configure
        .arg(format!("--prefix={}", Path::new(&unix_prefix).display()))
        .args([
            "--disable-everything",
            "--disable-autodetect",
            "--disable-programs",
            "--disable-doc",
            "--disable-htmlpages",
            "--disable-manpages",
            "--disable-podpages",
            "--disable-txtpages",
            "--disable-debug",
            "--disable-network",
            "--disable-shared",
            "--enable-static",
            "--enable-pic",
            "--disable-avdevice",
            "--disable-avfilter",
            "--enable-swscale",
            "--enable-libopenh264",
            "--enable-swresample",
            "--enable-protocol=file",
            "--enable-demuxer=mov,matroska,ogg,aac",
            "--enable-muxer=ipod,mp4,hls,mpegts",
            "--enable-decoder=aac,opus,vorbis,h264,vp9,hevc",
            "--enable-encoder=aac,libopenh264",
            "--enable-parser=aac,opus,vorbis,h264,vp9,hevc",
            "--enable-bsf=aac_adtstoasc,h264_mp4toannexb",
        ]))?;
    // The default install target copies doc/examples even with docs disabled.
    // These targets build and install the libraries, headers, and pkg-config files.
    let mut library_make = unix_command("make", msys2_bin.as_deref());
    library_make
        .current_dir(&source)
        .arg(format!("-j{}", std::thread::available_parallelism()?))
        .args(["install-libs", "install-headers"]);
    if let Some(path) = &native_path {
        library_make.env("PATH", path);
    }
    run(&mut library_make)?;
    fs::copy(
        source.join("COPYING.LGPLv2.1"),
        prefix.join("COPYING.LGPLv2.1"),
    )?;
    fs::write(
        prefix.join("NOTICE.txt"),
        format!(
            "FFmpeg {version}\nSource: {url}\nSHA-256: {checksum}\nConfiguration: tools/native-ffmpeg.rs\n{}",
            fs::read_to_string(source.join(Path::new("COPYING.LGPLv2.1")))?
                + &format!(
                    "\nOpenH264 {openh264_version}\nSource: {openh264_url}\nSHA-256: {openh264_checksum}\n{}",
                    fs::read_to_string(openh264_source.join("LICENSE"))?
                )
        ),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    #[cfg(windows)]
    fn msys_tar_extracts_into_windows_paths_with_spaces() {
        let bin = PathBuf::from(env::var_os("MSYS2_LOCATION").expect("MSYS2_LOCATION is required"))
            .join("usr/bin");
        let root = env::temp_dir().join(format!(
            "ffmpeg archive paths {} {}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root.join("source files");
        let destination = root.join("extracted files");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(source.join("fixture.txt"), "archive path regression").unwrap();
        run(unix_command("tar", Some(&bin)).current_dir(&root).args([
            "-cf",
            "fixture.tar",
            "-C",
            "source files",
            "fixture.txt",
        ]))
        .unwrap();
        extract_archive(&root.join("fixture.tar"), &destination, Some(&bin)).unwrap();
        assert_eq!(
            fs::read_to_string(destination.join("fixture.txt")).unwrap(),
            "archive path regression"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn msys_tools_ignore_ambient_search_paths() {
        let root = env::temp_dir().join(format!(
            "ffmpeg tool paths {} {}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let bin = root.join("msys64/usr/bin");
        let unrelated = root.join("unrelated");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&unrelated).unwrap();

        // The test binary is a real executable on every host. Selecting it must
        // work even with an empty PATH and spaces in the MSYS2 installation path.
        for program in ["bash", "make", "tar", "openssl"] {
            let executable = bin.join(format!("{program}.exe"));
            fs::copy(env::current_exe().unwrap(), &executable).unwrap();
            let output = unix_command(program, Some(&bin))
                .env("PATH", &unrelated)
                .current_dir(&unrelated)
                .arg("--list")
                .output()
                .unwrap_or_else(|error| panic!("Cannot launch MSYS2 {program}: {error}"));
            assert!(
                output.status.success(),
                "MSYS2 {program} failed: {output:?}"
            );
            assert!(
                String::from_utf8_lossy(&output.stdout)
                    .contains("tests::msys_tools_ignore_ambient_search_paths")
            );

            // An absent MSYS2 tool must fail, even if a namesake is on PATH.
            let namesake = unrelated.join(if cfg!(windows) {
                format!("{program}.exe")
            } else {
                program.to_owned()
            });
            fs::rename(&executable, namesake).unwrap();
            let error = unix_command(program, Some(&bin))
                .env("PATH", &unrelated)
                .arg("--list")
                .output()
                .unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        }
        fs::remove_dir_all(root).unwrap();
    }
}
