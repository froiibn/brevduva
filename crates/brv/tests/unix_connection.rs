// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0
#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

struct Probe {
    child: Child,
    dir: PathBuf,
}
impl Drop for Probe {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn worker_detaches_waits_for_app_and_stops_on_disconnect() {
    let dir = PathBuf::from("/tmp").join(format!(
        "brv-worker-{}",
        brevduva_protocol::ClientKey::generate()
    ));
    let state_dir = dir.join("desktop/legacy/agent/channel");
    std::fs::create_dir_all(&state_dir).unwrap();
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        "server = 'http://127.0.0.1:9'\n[[binding]]\nagent = 'agent'\nchannel = 'channel'\n",
    )
    .unwrap();
    let mut saved = serde_json::json!({"server":"http://127.0.0.1:9","binding":"agent@channel",
        "task":{"adapter":"codex-desktop","id":"test-task"},"generation":"test-generation","desired":"connected"});
    std::fs::write(state_dir.join("connection.json"), saved.to_string()).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_brv"))
        .args([
            "connection",
            "worker",
            "--binding",
            "agent@channel",
            "--generation",
            "test-generation",
        ])
        .env("BREVDUVA_CONFIG", &config)
        .env("CODEX_HOME", dir.join("no-desktop"))
        .env_remove("BREVDUVA_BINDING")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut probe = Probe { child, dir };
    let mut ready = false;
    for _ in 0..200 {
        if let Ok(bytes) = std::fs::read(state_dir.join("runtime.json")) {
            let state: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            if state["state"] == "waiting_for_app" {
                ready = true;
                break;
            }
        }
        assert!(
            probe.child.try_wait().unwrap().is_none(),
            "worker exited before app wait"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(ready, "worker did not enter app wait");
    let pid = rustix::process::Pid::from_raw(probe.child.id() as i32).unwrap();
    assert_eq!(rustix::process::getsid(Some(pid)).unwrap(), pid);
    saved["desired"] = serde_json::json!("disconnected");
    let temp = state_dir.join("new.json");
    std::fs::write(&temp, saved.to_string()).unwrap();
    std::fs::rename(temp, state_dir.join("connection.json")).unwrap();
    for _ in 0..100 {
        if let Some(status) = probe.child.try_wait().unwrap() {
            assert!(status.success());
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("worker did not stop cooperatively");
}
