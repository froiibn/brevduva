// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! Codex Desktop 작업 도우미 — 리시버가 **사용자 명의로** 실행한다(RECEIVER_REBUILD_PLAN 7d).
//! 앱의 내부 IPC(Windows `codex-ipc` 파이프, 유닉스 `CODEX_HOME/ipc/ipc.sock`)로 작업의 소유자를
//! 확인하고(`check`), 전달 하나마다 턴을 연다(`submit`). 서버에 붙지 않고 기록도 쓰지 않는다 —
//! 기록·확정은 리시버의 평면이 한다. 턴이 열린 것은 작업 완료가 아니다. 서버에 직접 붙어 자체
//! 기록을 쓰던 옛 사용자 명의 worker(`desktop run`·`brv connect`)는 2026-09-11 삭제(7e).

#[cfg(unix)]
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;
use brevduva_protocol::ClientKey;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

const RPC_TIMEOUT: Duration = Duration::from_secs(20);
/// 리시버 도우미가 앱이 바쁠 때 다시 시도하는 간격.
const BUSY_RETRY: Duration = Duration::from_secs(3);
const BUSY: &str = "App context must wait until the current turn finishes";
const FRAME_LIMIT: usize = 32 * 1024 * 1024;

struct Ipc<S> {
    stream: S,
    client_id: String,
}

#[derive(Debug)]
enum RpcError {
    Explicit(String),
    Unknown(String),
}
impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Explicit(s) | Self::Unknown(s) => f.write_str(s),
        }
    }
}
impl std::error::Error for RpcError {}
impl RpcError {
    fn busy(&self) -> bool {
        matches!(self, Self::Explicit(s) if s.contains(BUSY))
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Ipc<S> {
    async fn request(
        &mut self,
        method: &str,
        params: Value,
        version: u32,
        target: Option<&str>,
    ) -> Result<Value, RpcError> {
        let request_id = ClientKey::generate().to_string();
        let mut body = json!({"type":"request", "requestId":request_id, "sourceClientId":self.client_id,
            "version":version, "method":method, "params":params, "timeoutMs":RPC_TIMEOUT.as_millis() as u64});
        if let Some(target) = target {
            body["targetClientId"] = json!(target);
        }
        let bytes = serde_json::to_vec(&body).map_err(|e| RpcError::Unknown(e.to_string()))?;
        if bytes.len() > FRAME_LIMIT {
            return Err(RpcError::Unknown("IPC input exceeds frame limit".into()));
        }
        let response = tokio::time::timeout(RPC_TIMEOUT, async {
            self.stream.write_u32_le(bytes.len() as u32).await?;
            self.stream.write_all(&bytes).await?;
            self.stream.flush().await?;
            loop {
                let size = self.stream.read_u32_le().await? as usize;
                anyhow::ensure!(size <= FRAME_LIMIT, "IPC frame exceeds limit");
                let mut bytes = vec![0; size];
                self.stream.read_exact(&mut bytes).await?;
                let response: Value = serde_json::from_slice(&bytes)?;
                if response["type"] == "response" && response["requestId"] == request_id {
                    return Ok::<Value, anyhow::Error>(response);
                }
            }
        })
        .await
        .map_err(|_| RpcError::Unknown("IPC timed out; delivery outcome unknown".into()))?
        .map_err(|e| RpcError::Unknown(e.to_string()))?;
        if response["resultType"] != "success" {
            return Err(RpcError::Explicit(response.to_string()));
        }
        Ok(response)
    }

    async fn owner(&mut self, thread: &str) -> anyhow::Result<String> {
        let init = self
            .request(
                "initialize",
                json!({"clientType":"brevduva-receiver"}),
                0,
                None,
            )
            .await?;
        self.client_id = init["result"]["clientId"]
            .as_str()
            .context("IPC initialize lacks clientId")?
            .into();
        let owner = self
            .request(
                "thread-owner-discovery",
                json!({"hostId":"local", "conversationId":thread}),
                1,
                None,
            )
            .await?;
        anyhow::ensure!(
            owner["result"]["supportsUntrustedAppInput"] == true,
            "Desktop owner does not support external input"
        );
        Ok(owner["handledByClientId"]
            .as_str()
            .context("no existing Desktop owner")?
            .to_owned())
    }
}

#[cfg(windows)]
async fn open_ipc() -> anyhow::Result<Ipc<tokio::net::windows::named_pipe::NamedPipeClient>> {
    Ok(Ipc {
        stream: tokio::net::windows::named_pipe::ClientOptions::new()
            .open(r"\\.\pipe\codex-ipc")?,
        client_id: "initializing-client".into(),
    })
}

#[cfg(unix)]
fn unix_socket_path(codex_home: Option<PathBuf>, home: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    let root = codex_home
        .or_else(|| home.map(|h| h.join(".codex")))
        .context("cannot resolve Codex home")?;
    anyhow::ensure!(root.is_absolute(), "CODEX_HOME must be an absolute path");
    Ok(root.join("ipc").join("ipc.sock"))
}

#[cfg(unix)]
async fn connect_unix(path: &Path) -> anyhow::Result<Ipc<tokio::net::UnixStream>> {
    use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
    let uid = rustix::process::getuid().as_raw();
    let parent = std::fs::symlink_metadata(path.parent().context("IPC path has no parent")?)?;
    let socket = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(
        parent.is_dir() && parent.uid() == uid && parent.mode() & 0o022 == 0,
        "Codex IPC directory must belong to this user and not be group/world writable"
    );
    anyhow::ensure!(
        socket.file_type().is_socket() && socket.uid() == uid,
        "Codex IPC endpoint must be a socket owned by this user"
    );
    let stream = tokio::time::timeout(RPC_TIMEOUT, tokio::net::UnixStream::connect(path)).await??;
    // 검사 직후 소켓이 교체돼도 실제 접속 상대가 같은 사용자인지 다시 확인한다.
    anyhow::ensure!(
        stream.peer_cred()?.uid() == uid,
        "Codex IPC peer belongs to another user"
    );
    Ok(Ipc {
        stream,
        client_id: "initializing-client".into(),
    })
}

#[cfg(unix)]
async fn open_ipc() -> anyhow::Result<Ipc<tokio::net::UnixStream>> {
    let path = unix_socket_path(
        std::env::var_os("CODEX_HOME").map(PathBuf::from),
        dirs::home_dir(),
    )?;
    connect_unix(&path)
        .await
        .with_context(|| format!("no supported Desktop IPC at {}", path.display()))
}

#[cfg(not(any(windows, unix)))]
async fn open_ipc() -> anyhow::Result<Ipc<tokio::io::DuplexStream>> {
    anyhow::bail!("experimental Desktop delivery requires Windows or Unix")
}

pub(crate) fn validate_thread(thread: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !thread.is_empty()
            && thread.len() <= 128
            && thread
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "--thread must be the exact existing Desktop task ID"
    );
    Ok(())
}

pub async fn check(thread: &str) -> anyhow::Result<()> {
    validate_thread(thread)?;
    let owner = open_ipc().await?.owner(thread).await?;
    println!(
        "{}",
        json!({"thread":thread, "owner":owner, "external_input":true, "experimental":true})
    );
    Ok(())
}

/// 외부 앱 입력으로 작업에 턴을 여는 요청. 글은 앱의 신뢰하지 않는 입력 칸에 싣고, 작업 설정은
/// 작업의 것을 물려받는다 — 외부 글이 승인·작업 폴더를 바꾸지 못한다.
fn turn_start_params(thread: &str, text: &str) -> Value {
    let context = json!({"version":1, "message":{"source":"mcp_app", "sourceId":"brevduva-receiver", "text":text}});
    let prompt = "Respond to the user input in the context of our conversation.";
    let call_id = format!("brv_{}", ClientKey::generate());
    json!({"conversationId":thread, "turnStart":{
    "request":{"threadId":thread, "input":[{"type":"text", "text":prompt,
        "text_elements":[{"byteRange":{"start":0,"end":prompt.len()},
            "placeholder":format!("codex-untrusted-app-input:{context}")}]}]},
    "context":{"inheritThreadSettings":true,"responseItems":[
        {"type":"function_call","call_id":call_id,"name":"untrusted_input","arguments":"{}"},
        {"type":"function_call_output","call_id":call_id,"output":[{"type":"input_text","text":context["message"].to_string()}]}
    ]}}})
}

/// 리시버의 Desktop 통로 도우미 (2026-09-11, RECEIVER_REBUILD_PLAN 7d). 리시버가 **사용자 명의로**
/// 전달마다 한 번 실행한다 — 소유자를 다시 확인하고 그 작업에 턴을 연다. 넣는 글은 Monitor와 같은
/// 사건 한 줄이고 동료 본문은 없다(본문은 receipt 도구 결과로만). 서버에 붙지 않고 기록도 쓰지
/// 않는다 — 기록과 확정은 리시버가 한다. 결과는 표준 출력의 JSON 한 줄이다:
/// `started`(turn id) · `busy`(앱이 턴 처리 중, 넣지 않음) · `not_submitted`(보내기 전 실패) ·
/// `unknown`(보냈으나 결과 불명 — 자동 재시도 금지).
pub async fn submit(
    thread: &str,
    message_id: &str,
    receipt: &str,
    busy_wait: Duration,
) -> anyhow::Result<()> {
    let outcome = submit_with(open_ipc, thread, message_id, receipt, busy_wait, BUSY_RETRY).await;
    println!("{outcome}");
    Ok(())
}

async fn submit_with<S, F, Fut>(
    mut open: F,
    thread: &str,
    message_id: &str,
    receipt: &str,
    busy_wait: Duration,
    retry: Duration,
) -> Value
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Ipc<S>>>,
{
    let not_submitted = |error: anyhow::Error| json!({"desktop_submit":"not_submitted","message":format!("{error:#}")});
    if let Err(error) = validate_thread(thread) {
        return not_submitted(error);
    }
    let text =
        crate::session_delivery::monitor_event(&json!(message_id), &json!(receipt)).to_string();
    let deadline = tokio::time::Instant::now() + busy_wait;
    loop {
        // 턴 요청 전 단계(연결·소유자 확인)의 실패는 넣지 않은 것이 확실하다.
        let mut ipc = match open().await {
            Ok(ipc) => ipc,
            Err(error) => return not_submitted(error),
        };
        let owner = match ipc.owner(thread).await {
            Ok(owner) => owner,
            Err(error) => return not_submitted(error),
        };
        match ipc
            .request(
                "thread-follower-start-turn",
                turn_start_params(thread, &text),
                2,
                Some(&owner),
            )
            .await
        {
            Ok(response) => {
                return match response["result"]["result"]["turn"]["id"].as_str() {
                    Some(turn) => json!({"desktop_submit":"started","turn_id":turn}),
                    None => {
                        json!({"desktop_submit":"unknown","message":"success response missing turn ID"})
                    }
                };
            }
            Err(error) if error.busy() => {
                if tokio::time::Instant::now() + retry > deadline {
                    return json!({"desktop_submit":"busy","message":"the Desktop task is still running its current turn"});
                }
                tokio::time::sleep(retry).await;
            }
            // 명시적 거절도 턴이 열리지 않았다고 단정하지 않는다 — 옛 경로와 같이 결과 불명.
            Err(error) => {
                return json!({"desktop_submit":"unknown","message":error.to_string()});
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    struct Fixture(PathBuf);
    #[cfg(unix)]
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn external_text_does_not_override_local_task_settings() {
        let params = turn_start_params("chosen-task", "peer text");
        assert_eq!(params["conversationId"], "chosen-task");
        assert_eq!(
            params["turnStart"]["context"]["inheritThreadSettings"],
            true
        );
        let request = &params["turnStart"]["request"];
        assert!(request.get("approvalPolicy").is_none());
        assert!(request.get("cwd").is_none());
        assert!(
            !request["input"][0]["text"]
                .as_str()
                .unwrap()
                .contains("peer text")
        );
        assert!(
            request["input"][0]["text_elements"][0]["placeholder"]
                .as_str()
                .unwrap()
                .starts_with("codex-untrusted-app-input:")
        );
    }

    #[test]
    fn only_explicit_busy_rejection_can_retry() {
        assert!(RpcError::Explicit(BUSY.into()).busy());
        assert!(!RpcError::Unknown(BUSY.into()).busy());
        assert!(!RpcError::Explicit("invalid input".into()).busy());
    }

    async fn read_request<S: AsyncRead + Unpin>(stream: &mut S) -> Value {
        let size = stream.read_u32_le().await.unwrap() as usize;
        let mut bytes = vec![0; size];
        stream.read_exact(&mut bytes).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
    async fn send_response<S: AsyncWrite + Unpin>(stream: &mut S, request: &Value, result: Value) {
        let bytes = serde_json::to_vec(&json!({"type":"response","requestId":request["requestId"],
            "resultType":"success","handledByClientId":"owner-a","result":result}))
        .unwrap();
        // 프레임 분할을 실제 스트림으로 검증한다.
        stream.write_u32_le(bytes.len() as u32).await.unwrap();
        for chunk in bytes.chunks(7) {
            stream.write_all(chunk).await.unwrap();
        }
    }

    #[tokio::test]
    async fn ipc_discovers_owner_and_sends_to_exact_live_task() {
        let (client, mut server) = tokio::io::duplex(8192);
        let peer = tokio::spawn(async move {
            let init = read_request(&mut server).await;
            assert_eq!(init["method"], "initialize");
            send_response(&mut server, &init, json!({"clientId":"receiver"})).await;
            let discover = read_request(&mut server).await;
            assert_eq!(discover["params"]["conversationId"], "task-a");
            send_response(
                &mut server,
                &discover,
                json!({"supportsUntrustedAppInput":true}),
            )
            .await;
            let start = read_request(&mut server).await;
            assert_eq!(start["method"], "thread-follower-start-turn");
            assert_eq!(start["targetClientId"], "owner-a");
            assert_eq!(start["version"], 2);
            send_response(
                &mut server,
                &start,
                json!({"result":{"turn":{"id":"turn-a"}}}),
            )
            .await;
        });
        let mut ipc = Ipc {
            stream: client,
            client_id: "initializing-client".into(),
        };
        let owner = ipc.owner("task-a").await.unwrap();
        let response = ipc
            .request("thread-follower-start-turn", json!({}), 2, Some(&owner))
            .await
            .unwrap();
        assert_eq!(response["result"]["result"]["turn"]["id"], "turn-a");
        peer.await.unwrap();
    }

    /// 리시버 도우미가 만날 Desktop 소유자의 턴 요청 응답.
    #[derive(Clone, Copy)]
    enum OwnerAnswer {
        Turn,
        Busy,
        NoExternalInput,
        HangUp,
    }

    /// 연결 하나를 흉내 낸다 — 턴 요청을 받았으면 그 요청을 돌려준다.
    fn desktop_peer(
        answer: OwnerAnswer,
    ) -> (
        Ipc<tokio::io::DuplexStream>,
        tokio::task::JoinHandle<Option<Value>>,
    ) {
        let (client, mut server) = tokio::io::duplex(16 * 1024);
        let peer = tokio::spawn(async move {
            let init = read_request(&mut server).await;
            send_response(&mut server, &init, json!({"clientId":"receiver"})).await;
            let discover = read_request(&mut server).await;
            let external = !matches!(answer, OwnerAnswer::NoExternalInput);
            send_response(
                &mut server,
                &discover,
                json!({"supportsUntrustedAppInput": external}),
            )
            .await;
            if !external {
                return None;
            }
            let start = read_request(&mut server).await;
            match answer {
                OwnerAnswer::Turn => {
                    send_response(
                        &mut server,
                        &start,
                        json!({"result":{"turn":{"id":"turn-7"}}}),
                    )
                    .await
                }
                OwnerAnswer::Busy => {
                    let bytes = serde_json::to_vec(&json!({"type":"response",
                        "requestId":start["requestId"],"resultType":"error","error":BUSY}))
                    .unwrap();
                    server.write_u32_le(bytes.len() as u32).await.unwrap();
                    server.write_all(&bytes).await.unwrap();
                }
                OwnerAnswer::HangUp | OwnerAnswer::NoExternalInput => {}
            }
            Some(start)
        });
        (
            Ipc {
                stream: client,
                client_id: "initializing-client".into(),
            },
            peer,
        )
    }

    async fn helper_outcome(
        connections: Vec<Ipc<tokio::io::DuplexStream>>,
        thread: &str,
        busy_wait: Duration,
    ) -> Value {
        let mut connections = connections.into_iter();
        submit_with(
            || {
                let next = connections.next();
                async move { next.context("no further Desktop connection") }
            },
            thread,
            "msg-1",
            "token-1",
            busy_wait,
            Duration::from_millis(5),
        )
        .await
    }

    #[tokio::test]
    async fn the_receiver_helper_opens_a_turn_carrying_only_the_receipt_event() {
        let (ipc, peer) = desktop_peer(OwnerAnswer::Turn);
        let outcome = helper_outcome(vec![ipc], "task-a", Duration::ZERO).await;
        assert_eq!(
            outcome,
            json!({"desktop_submit":"started","turn_id":"turn-7"})
        );
        let start = peer.await.unwrap().expect("turn request");
        assert_eq!(start["targetClientId"], "owner-a");
        assert_eq!(start["params"]["conversationId"], "task-a");
        let turn = start["params"]["turnStart"].to_string();
        assert!(turn.contains("token-1") && turn.contains("msg-1"), "{turn}");
        assert!(turn.contains("brevduva_message"), "{turn}");
        assert!(
            !turn.contains("payload") && !turn.contains("provenance"),
            "동료 본문(봉투)은 넣지 않는다 — receipt로만: {turn}"
        );
        assert_eq!(
            start["params"]["turnStart"]["context"]["inheritThreadSettings"],
            true
        );
    }

    #[tokio::test]
    async fn the_receiver_helper_waits_out_a_busy_task_then_reports_busy() {
        let (busy, _first) = desktop_peer(OwnerAnswer::Busy);
        let (ready, second) = desktop_peer(OwnerAnswer::Turn);
        let outcome = helper_outcome(vec![busy, ready], "task-a", Duration::from_secs(5)).await;
        assert_eq!(outcome["desktop_submit"], "started");
        assert!(second.await.unwrap().is_some());

        let (busy, _peer) = desktop_peer(OwnerAnswer::Busy);
        let outcome = helper_outcome(vec![busy], "task-a", Duration::ZERO).await;
        assert_eq!(
            outcome["desktop_submit"], "busy",
            "넣지 않았다고 확실히 말한다"
        );
    }

    #[tokio::test]
    async fn the_receiver_helper_separates_not_sent_from_unknown() {
        let (ipc, peer) = desktop_peer(OwnerAnswer::NoExternalInput);
        let outcome = helper_outcome(vec![ipc], "task-a", Duration::ZERO).await;
        assert_eq!(outcome["desktop_submit"], "not_submitted", "{outcome}");
        assert!(peer.await.unwrap().is_none(), "턴 요청을 보내지 않았다");

        let outcome = helper_outcome(Vec::new(), "task-a", Duration::ZERO).await;
        assert_eq!(outcome["desktop_submit"], "not_submitted");

        let outcome = helper_outcome(Vec::new(), "../other", Duration::ZERO).await;
        assert_eq!(outcome["desktop_submit"], "not_submitted");

        let (ipc, _peer) = desktop_peer(OwnerAnswer::HangUp);
        let outcome = helper_outcome(vec![ipc], "task-a", Duration::ZERO).await;
        assert_eq!(
            outcome["desktop_submit"], "unknown",
            "보낸 뒤 끊기면 결과 불명: {outcome}"
        );
    }

    #[tokio::test]
    async fn ipc_disconnect_after_request_is_unknown() {
        let (client, mut server) = tokio::io::duplex(8192);
        let peer = tokio::spawn(async move {
            read_request(&mut server).await;
        });
        let mut ipc = Ipc {
            stream: client,
            client_id: "client".into(),
        };
        assert!(matches!(
            ipc.request("thread-follower-start-turn", json!({}), 2, None)
                .await,
            Err(RpcError::Unknown(_))
        ));
        peer.await.unwrap();
    }

    #[cfg(unix)]
    fn unix_fixture() -> Fixture {
        use std::os::unix::fs::PermissionsExt as _;
        let path = PathBuf::from("/tmp").join(format!("brv-ipc-{}", ClientKey::generate()));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        Fixture(path)
    }

    #[cfg(unix)]
    #[test]
    fn unix_path_uses_codex_home_without_scanning_other_sessions() {
        assert_eq!(
            unix_socket_path(Some("/custom".into()), Some("/home/u".into())).unwrap(),
            PathBuf::from("/custom/ipc/ipc.sock")
        );
        assert_eq!(
            unix_socket_path(None, Some("/home/u".into())).unwrap(),
            PathBuf::from("/home/u/.codex/ipc/ipc.sock")
        );
        assert!(unix_socket_path(Some("relative".into()), None).is_err());
        assert!(unix_socket_path(None, None).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_socket_runs_same_owner_and_turn_protocol() {
        let fixture = unix_fixture();
        let path = fixture.0.join("s");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let peer = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let init = read_request(&mut stream).await;
            send_response(&mut stream, &init, json!({"clientId":"receiver"})).await;
            let discover = read_request(&mut stream).await;
            assert_eq!(discover["params"]["conversationId"], "unix-task");
            send_response(
                &mut stream,
                &discover,
                json!({"supportsUntrustedAppInput":true}),
            )
            .await;
            let start = read_request(&mut stream).await;
            assert_eq!(start["targetClientId"], "owner-a");
            assert_eq!(start["method"], "thread-follower-start-turn");
            send_response(
                &mut stream,
                &start,
                json!({"result":{"turn":{"id":"unix-turn"}}}),
            )
            .await;
        });
        let mut ipc = connect_unix(&path).await.unwrap();
        let owner = ipc.owner("unix-task").await.unwrap();
        let result = ipc
            .request("thread-follower-start-turn", json!({}), 2, Some(&owner))
            .await
            .unwrap();
        assert_eq!(result["result"]["result"]["turn"]["id"], "unix-turn");
        peer.await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_socket_rejects_writable_directory_and_symlink() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};
        let fixture = unix_fixture();
        let path = fixture.0.join("s");
        let _listener = tokio::net::UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&fixture.0, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(connect_unix(&path).await.is_err());
        std::fs::set_permissions(&fixture.0, std::fs::Permissions::from_mode(0o700)).unwrap();
        let alias = fixture.0.join("alias");
        symlink(&path, &alias).unwrap();
        assert!(connect_unix(&alias).await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_stale_socket_fails_without_removing_the_endpoint() {
        let fixture = unix_fixture();
        let path = fixture.0.join("s");
        drop(tokio::net::UnixListener::bind(&path).unwrap());
        assert!(connect_unix(&path).await.is_err());
        assert!(path.exists());
    }
}
