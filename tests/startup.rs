#[test]
fn malformed_dotenv_reports_context_and_fails() {
    // Isolated subprocess: never read the developer's .env or mutate the test process environment.
    let directory =
        std::env::temp_dir().join(format!("metamatch-startup-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(
        directory.join(".env"),
        "ENSO_API_KEY='fixture-startup-secret\n",
    )
    .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_metamatch-backend"))
        .current_dir(&directory)
        .env_clear()
        .output();
    std::fs::remove_dir_all(&directory).unwrap();
    let output = output.unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("INVALID_CONFIG"));
    assert!(stderr.contains("loading .env"));
    assert!(stderr.contains("fixture-startup-secret"));
    assert!(output.stdout.is_empty());
}
