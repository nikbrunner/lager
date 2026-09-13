mod support;

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::{Duration, Instant};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn flag_driven_init_reports_missing_tools_but_creates_config() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let bin = temp.path().join("empty-bin");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home)?;
    fs::create_dir_all(&bin)?;

    let output = support::lager_with_exact_path(&home, &config, &bin)
        .args(["init", "--root", "repos", "--create-root", "--github"])
        .output()?;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(config.is_file());
    assert!(home.join("repos").is_dir());
    let stderr = String::from_utf8(output.stderr)?;
    for diagnostic in [
        "git is not available",
        "fzf is not available",
        "authenticated gh",
    ] {
        assert!(
            stderr.contains(diagnostic),
            "missing diagnostic: {diagnostic}"
        );
    }
    assert!(!stderr.contains('\u{1b}'));
    Ok(())
}

#[test]
fn flag_driven_register_preserves_the_exact_hook_command() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home)?;
    fs::write(&config, "root = \"repos\"\n")?;
    let command = "  make setup && printf 'x y'  ";

    let output = support::lager(&home, &config)
        .args(["register", "github.com/org/repo", "--post-clone", command])
        .output()?;

    assert!(output.status.success());
    let document = fs::read_to_string(config)?;
    assert!(document.contains("post_clone = \"  make setup && printf 'x y'  \""));
    assert!(!String::from_utf8_lossy(&output.stdout).contains('\u{1b}'));
    assert!(!String::from_utf8_lossy(&output.stderr).contains('\u{1b}'));
    Ok(())
}

#[test]
fn non_tty_json_output_remains_one_undecorated_document() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home)?;
    fs::write(&config, "root = \"repos\"\n")?;

    let output = support::lager(&home, &config)
        .args(["list", "--json"])
        .output()?;

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(!output.stdout.contains(&0x1b));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(value["schema_version"], 1);
    Ok(())
}

#[test]
fn release_workflow_pins_cross_and_verifies_source_and_architectures() -> TestResult {
    let workflow = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/.github/workflows/release.yml"
    ))?;
    assert!(workflow.contains("--rev 88f49ff79e777bef6d3564531636ee4d3cc2f8d2 --locked"));
    assert!(!workflow.contains("--tag v0.2.5"));
    for proof in [
        "git describe --tags --exact-match HEAD",
        "cargo metadata --locked --no-deps",
        "cargo run --locked --quiet -- --version",
        "gh release upload \"$tag\" dist/* --clobber --repo \"$GITHUB_REPOSITORY\"",
        "aarch64-apple-darwin) printf",
        "x86_64-apple-darwin) printf",
        "aarch64-unknown-linux-musl) printf",
        "x86_64-unknown-linux-musl) printf",
    ] {
        assert!(workflow.contains(proof), "missing release proof: {proof}");
    }
    Ok(())
}

#[test]
fn observable_outbound_tripwire_records_remote_access_and_localhost_bypasses_it() -> TestResult {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let config = temp.path().join("config.toml");
    fs::create_dir_all(&home)?;

    let local = TcpListener::bind("127.0.0.1:0")?;
    let local_url = format!("http://{}", local.local_addr()?);
    let local_server = thread::spawn(move || {
        let (mut stream, _) = local.accept().unwrap();
        let mut request = [0_u8; 2048];
        let _ = stream.read(&mut request).unwrap();
        let body = r#"{"values":[]}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    fs::write(
        &config,
        format!(
            "root = \"repos\"\n\n[providers.\"bitbucket.org\"]\npreset = \"bitbucket-cloud\"\napi_url = \"{local_url}\"\n\n[[repositories]]\nurl = \"bitbucket.org/workspace/*\"\n"
        ),
    )?;
    support::clear_outbound_requests();
    let local_output = support::lager(&home, &config)
        .args(["list", "--remote", "--json"])
        .output()?;
    local_server.join().unwrap();
    assert!(
        local_output.status.success(),
        "{}",
        String::from_utf8_lossy(&local_output.stderr)
    );
    assert!(support::outbound_requests().is_empty());

    fs::write(
        &config,
        "root = \"repos\"\n\n[providers.\"bitbucket.org\"]\npreset = \"bitbucket-cloud\"\napi_url = \"https://example.invalid\"\n\n[[repositories]]\nurl = \"bitbucket.org/workspace/*\"\n",
    )?;
    support::clear_outbound_requests();
    let started = Instant::now();
    let output = support::lager(&home, &config)
        .args(["list", "--remote", "--json"])
        .output()?;
    assert_eq!(output.status.code(), Some(1));
    assert!(started.elapsed() < Duration::from_secs(5));

    let deadline = Instant::now() + Duration::from_secs(2);
    let requests = loop {
        let requests = support::outbound_requests();
        if !requests.is_empty() || Instant::now() >= deadline {
            break requests;
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(requests.len(), 1, "tripwire requests: {requests:?}");
    assert!(
        requests[0].starts_with("CONNECT example.invalid:443 "),
        "{}",
        requests[0]
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("error sending request"));
    Ok(())
}
