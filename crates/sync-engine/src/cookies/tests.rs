use super::*;
use std::sync::{Arc, Barrier};

fn export(value: &str) -> String {
    format!("# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tSAPISID\t{value}\n")
}
fn url() -> Url {
    Url::parse("https://www.youtube.com/youtubei/v1/player").unwrap()
}
fn fixture() -> (tempfile::TempDir, CookieJar) {
    let directory = tempfile::tempdir().unwrap();
    let jar = CookieJar::new(directory.path());
    (directory, jar)
}

#[test]
fn validates_exports_without_disclosing_values_or_replacing_good_state() {
    let (_dir, jar) = fixture();
    jar.import(&export("private-value").replace('\n', "\r\n"))
        .unwrap();
    for input in [
        "[{\"value\":\"private-value\"}]".into(),
        export("private-value; other=oops"),
        export("private-value\rinjected"),
        export("private-value").replace("TRUE", "maybe"),
    ] {
        let message = jar.import(&input).unwrap_err().to_string();
        assert!(!message.contains("private-value"));
        assert_eq!(
            jar.snapshot().unwrap().header(&url()),
            "SAPISID=private-value"
        );
    }
    assert!(jar.import(&"x".repeat(MAX_BYTES + 1)).is_err());
}

#[test]
fn respects_host_path_secure_expiry_and_http_only() {
    let (_dir, jar) = fixture();
    let input = concat!(
        "# HTTP Cookie File\n",
        "#HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tsecure\n",
        "www.youtube.com\tFALSE\t/youtubei\tFALSE\t0\tHOST\tonly\n",
        ".youtube.com\tTRUE\t/\tTRUE\t1\tOLD\texpired\n",
        ".example.com\tTRUE\t/\tTRUE\t0\tOTHER\tignored,elsewhere\n"
    );
    jar.import(input).unwrap();
    let snapshot = jar.snapshot().unwrap();
    assert_eq!(snapshot.header(&url()), "HOST=only; SID=secure");
    assert_eq!(
        snapshot.header(&Url::parse("https://m.youtube.com/").unwrap()),
        "SID=secure"
    );
    assert_eq!(
        snapshot.header(&Url::parse("https://www.youtube.com/youtubeix").unwrap()),
        "SID=secure"
    );
    assert!(
        snapshot
            .header(&Url::parse("http://m.youtube.com/").unwrap())
            .is_empty()
    );
    assert!(
        snapshot
            .header(&Url::parse("https://youtube.com.example.com/").unwrap())
            .is_empty()
    );
    assert!(
        snapshot
            .header(&Url::parse("https://media.googlevideo.com/").unwrap())
            .is_empty()
    );
}

#[test]
fn merges_concurrent_updates_without_stale_overwrites_or_resurrection() {
    let (_dir, jar) = fixture();
    jar.import(&export("seed")).unwrap();
    let initial = jar.snapshot().unwrap();
    let gate = Arc::new(Barrier::new(3));
    std::thread::scope(|scope| {
        for header in [
            "FIRST=one; Domain=youtube.com; Path=/; Secure",
            "SECOND=two; Domain=youtube.com; Path=/; Secure",
        ] {
            let (jar, initial, gate) = (jar.clone(), initial.clone(), gate.clone());
            scope.spawn(move || {
                gate.wait();
                jar.update(&initial, &url(), &[header.into()]).unwrap();
            });
        }
        gate.wait();
    });
    let both = jar.snapshot().unwrap().header(&url());
    assert!(both.contains("FIRST=one") && both.contains("SECOND=two"));
    jar.update(
        &initial,
        &url(),
        &["SAPISID=new; Domain=youtube.com; Path=/; Secure".into()],
    )
    .unwrap();
    jar.update(
        &initial,
        &url(),
        &["SAPISID=stale; Domain=youtube.com; Path=/; Secure".into()],
    )
    .unwrap();
    assert!(
        jar.snapshot()
            .unwrap()
            .header(&url())
            .contains("SAPISID=new")
    );
    let before_delete = jar.snapshot().unwrap();
    jar.update(
        &before_delete,
        &url(),
        &["SAPISID=; Max-Age=0; Domain=youtube.com; Path=/; Secure".into()],
    )
    .unwrap();
    jar.update(
        &before_delete,
        &url(),
        &["SAPISID=resurrected; Domain=youtube.com; Path=/; Secure".into()],
    )
    .unwrap();
    assert!(!jar.snapshot().unwrap().header(&url()).contains("SAPISID="));
    jar.import(&export("replacement")).unwrap();
    jar.update(
        &initial,
        &url(),
        &["SAPISID=late; Domain=youtube.com; Path=/; Secure".into()],
    )
    .unwrap();
    assert_eq!(
        jar.snapshot().unwrap().header(&url()),
        "SAPISID=replacement"
    );
    let before_remove = jar.snapshot().unwrap();
    jar.remove().unwrap();
    jar.update(
        &before_remove,
        &url(),
        &["SAPISID=late; Domain=youtube.com; Path=/; Secure".into()],
    )
    .unwrap();
    assert!(!jar.snapshot().unwrap().enabled);
}

#[test]
fn readers_do_not_wait_for_writer_and_ignore_uncommitted_temporary_files() {
    let (directory, jar) = fixture();
    jar.import(&export("committed")).unwrap();
    let _lock = jar.write_lock().unwrap();
    std::fs::write(directory.path().join("youtube-cookies/incomplete.tmp"), "{").unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let reader = jar.clone();
    let thread = std::thread::spawn(move || {
        send.send(reader.snapshot().unwrap().header(&url()))
            .unwrap()
    });
    assert_eq!(
        receive
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap(),
        "SAPISID=committed"
    );
    thread.join().unwrap();
}

#[test]
fn rejects_foreign_response_cookies_and_persists_rotation_across_reopen() {
    let (directory, jar) = fixture();
    jar.import(&export("seed")).unwrap();
    let snapshot = jar.snapshot().unwrap();
    jar.update(
        &snapshot,
        &url(),
        &[
            "SAPISID=rotated; Domain=youtube.com; Path=/; Secure; Max-Age=3600".into(),
            "FOREIGN=no; Domain=com; Path=/".into(),
            "OTHER=no; Domain=example.com; Path=/".into(),
        ],
    )
    .unwrap();
    let reopened = CookieJar::new(directory.path()).snapshot().unwrap();
    assert_eq!(reopened.header(&url()), "SAPISID=rotated");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(jar.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn process_writer() {
    let Some(profile) = std::env::var_os("LISTENBOX_COOKIE_TEST_PROFILE") else {
        return;
    };
    let jar = CookieJar::new(Path::new(&profile));
    let base = jar.snapshot().unwrap();
    use std::io::Write;
    println!("COOKIE_WRITER_READY");
    std::io::stdout().flush().unwrap();
    let mut gate = String::new();
    std::io::stdin().read_line(&mut gate).unwrap();
    assert_eq!(gate.trim(), "commit");
    jar.update(
        &base,
        &url(),
        &[
            "SAPISID=stale-process; Domain=youtube.com; Path=/; Secure".into(),
            "PROCESS=merged; Domain=youtube.com; Path=/; Secure".into(),
        ],
    )
    .unwrap();
}

#[test]
fn process_updates_compare_against_committed_revisions() {
    use std::{
        io::{BufRead, Write},
        process::{Command, Stdio},
    };
    let (directory, jar) = fixture();
    jar.import(&export("seed")).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "cookies::tests::process_writer", "--nocapture"])
        .env("LISTENBOX_COOKIE_TEST_PROFILE", directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (ready, received) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            if line.unwrap() == "COOKIE_WRITER_READY" {
                ready.send(()).unwrap();
            }
        }
    });
    if received
        .recv_timeout(std::time::Duration::from_secs(3))
        .is_err()
    {
        let _ = child.kill();
        let _ = child.wait();
        panic!("child did not establish a cookie snapshot");
    }
    let base = jar.snapshot().unwrap();
    jar.update(
        &base,
        &url(),
        &["SAPISID=new-process; Domain=youtube.com; Path=/; Secure".into()],
    )
    .unwrap();
    child.stdin.take().unwrap().write_all(b"commit\n").unwrap();
    assert!(child.wait().unwrap().success());
    reader.join().unwrap();
    let header = jar.snapshot().unwrap().header(&url());
    assert!(header.contains("SAPISID=new-process") && header.contains("PROCESS=merged"));
    assert!(!header.contains("stale-process"));
}
