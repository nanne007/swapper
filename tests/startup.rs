#[test]
fn invalid_or_missing_config_fails_before_listening() {
    // Isolated subprocess: never read local credentials or mutate global environment.
    let directory =
        std::env::temp_dir().join(format!("metamatch-startup-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    for content in [
        None,
        Some("{"),
        Some(r#"{"port":0}"#),
        Some(r#"{"unknown":true}"#),
    ] {
        if let Some(content) = content {
            std::fs::write(directory.join("config.json"), content).unwrap();
        }
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_metamatch-backend"))
            .current_dir(&directory)
            .env_clear()
            .env("PORT", "3000")
            .output()
            .unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("INVALID_CONFIG"), "{stderr}");
        assert!(stderr.contains("config.json"));
        assert!(output.stdout.is_empty());
    }
    std::fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn explicit_config_path_is_used_without_dotenv_or_env_overrides() {
    let directory =
        std::env::temp_dir().join(format!("metamatch-startup-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join(".env"), "this is not a valid dotenv file").unwrap();
    std::fs::write(directory.join("custom.json"), r#"{"port":0}"#).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_metamatch-backend"))
        .current_dir(&directory)
        .env_clear()
        .env("PORT", "3000")
        .arg("custom.json")
        .output()
        .unwrap();
    std::fs::remove_dir_all(&directory).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!output.status.success());
    assert!(stderr.contains("custom.json"), "{stderr}");
    assert!(stderr.contains("port must be between"), "{stderr}");
    assert!(!stderr.contains(".env"));
}
