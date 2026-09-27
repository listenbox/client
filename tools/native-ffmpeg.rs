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
    if !archive.exists() {
        run(Command::new("curl")
            .args(["--fail", "--location", "--max-time", "60", "--output"])
            .arg(&archive)
            .arg(&url))?;
    }
    let digest = unix_command("openssl", msys2_bin.as_deref())
        .args(["dgst", "-sha256"])
        .arg(unix_path(&archive, msys2_bin.as_deref())?)
        .output()?;
    if !digest.status.success() || !String::from_utf8(digest.stdout)?.trim().ends_with(checksum) {
        return Err("FFmpeg source checksum mismatch".into());
    }
    extract_archive(&archive, &build, msys2_bin.as_deref())?;
    let source = build.join(format!("ffmpeg-{version}"));
    let mut configure = unix_command("bash", msys2_bin.as_deref());
    configure.arg("./configure").current_dir(&source);
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
            "--disable-swscale",
            "--enable-swresample",
            "--enable-protocol=file",
            "--enable-demuxer=mov,matroska,ogg,aac",
            "--enable-muxer=ipod,mp4,hls,mpegts",
            "--enable-decoder=aac,opus,vorbis,h264",
            "--enable-encoder=aac",
            "--enable-parser=aac,opus,vorbis,h264",
            "--enable-bsf=aac_adtstoasc,h264_mp4toannexb",
            "--disable-x86asm",
        ]))?;
    // The default install target copies doc/examples even with docs disabled.
    // These targets build and install the libraries, headers, and pkg-config files.
    run(unix_command("make", msys2_bin.as_deref())
        .current_dir(&source)
        .arg(format!("-j{}", std::thread::available_parallelism()?))
        .args(["install-libs", "install-headers"]))?;
    fs::copy(
        source.join("COPYING.LGPLv2.1"),
        prefix.join("COPYING.LGPLv2.1"),
    )?;
    fs::write(
        prefix.join("NOTICE.txt"),
        format!(
            "FFmpeg {version}\nSource: {url}\nSHA-256: {checksum}\nConfiguration: tools/native-ffmpeg.rs\n{}",
            fs::read_to_string(source.join(Path::new("COPYING.LGPLv2.1")))?
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
