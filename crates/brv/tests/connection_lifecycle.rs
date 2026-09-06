// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

use std::process::Command;

#[test]
fn installer_restart_handles_fresh_install_and_preserves_inactive_connections() {
    let dir = std::env::temp_dir().join(format!(
        "brv-lifecycle-{}",
        brevduva_protocol::ClientKey::generate()
    ));
    std::fs::create_dir(&dir).unwrap();
    let config = dir.join("config.toml");
    let restart = || {
        Command::new(env!("CARGO_BIN_EXE_brv"))
            .args(["connection", "restart"])
            .env("BREVDUVA_CONFIG", &config)
            .env_remove("BREVDUVA_BINDING")
            .output()
            .unwrap()
    };
    let fresh = restart();
    assert!(
        fresh.status.success(),
        "{}",
        String::from_utf8_lossy(&fresh.stderr)
    );
    assert!(!config.exists());
    std::fs::write(
        &config,
        "server='http://127.0.0.1:9'\n[[binding]]\nagent='agent'\nchannel='channel'\n",
    )
    .unwrap();
    let state_dir = dir.join("desktop/legacy/agent/channel");
    std::fs::create_dir_all(&state_dir).unwrap();
    let path = state_dir.join("connection.json");
    for desired in ["paused", "disconnected"] {
        let original = serde_json::json!({"server":"http://127.0.0.1:9", "binding":"agent@channel",
            "task":{"adapter":"codex-desktop","id":"test-task"},"generation":"original", "desired":desired}).to_string();
        std::fs::write(&path, &original).unwrap();
        assert!(restart().status.success());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert!(!state_dir.join("worker.lock").exists());
    }
    std::fs::write(&config, "invalid configuration").unwrap();
    assert!(!restart().status.success());
    std::fs::remove_dir_all(dir).unwrap();
}
