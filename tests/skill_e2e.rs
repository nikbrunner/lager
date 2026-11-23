use std::fs;

mod support;

#[test]
fn skill_prints_the_guide() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    fs::create_dir_all(&home)?;

    let output = support::lager(&home, &temp.path().join("config.toml"))
        .arg("skill")
        .output()?;

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout)?,
        fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/cli/skill/guide.md"
        ))?
    );
    assert!(output.stderr.is_empty());
    assert!(!home.join(".agents").exists());
    Ok(())
}

#[test]
fn skill_install_writes_the_stub_and_setup_guide_over_existing_files()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("home");
    let target = home.join(".agents/skills/lager");
    fs::create_dir_all(&target)?;
    fs::write(target.join("SKILL.md"), "stale")?;

    let output = support::lager(&home, &temp.path().join("config.toml"))
        .args(["skill", "install"])
        .output()?;

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout)?,
        format!("Installed skill to {}\n", target.display())
    );
    let source = concat!(env!("CARGO_MANIFEST_DIR"), "/skills/lager/");
    for file in ["SKILL.md", "references/setup.md"] {
        assert_eq!(
            fs::read_to_string(target.join(file))?,
            fs::read_to_string(format!("{source}{file}"))?,
            "{file}"
        );
    }
    assert!(fs::read_to_string(target.join("SKILL.md"))?.contains("lager skill"));
    Ok(())
}

#[test]
fn skill_install_without_home_fails() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let output = support::lager(temp.path(), &temp.path().join("config.toml"))
        .args(["skill", "install"])
        .env_remove("HOME")
        .output()?;

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8(output.stderr)?.contains("HOME is not set"));
    Ok(())
}
