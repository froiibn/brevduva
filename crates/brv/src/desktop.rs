// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! 기존 Desktop 작업 전달 실험 어댑터. 공개 프로토콜·헤드리스 데몬과 독립이다.
//! 사용자 계정으로 실행하며 내부 IPC에 의존한다. accepted는 작업 완료가 아니다.
//! 디스크 인계 후 ACK, 전송 전 submitting 기록, 불명확하면 재실행 없이 종료한다.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Seek as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use brevduva_protocol::{ClientKey, Envelope, Kind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::sync::{Mutex, Notify};

use crate::client::{Client, ClientOptions, RecvFilter};
use crate::config::{self, Binding, BrvConfig};

const RPC_TIMEOUT: Duration = Duration::from_secs(20);
const BUSY: &str = "App context must wait until the current turn finishes";
const FRAME_LIMIT: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Identity {
    server: String,
    binding: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DeliveryState {
    Pending,
    Submitting,
    Accepted,
    Unknown,
    Ignored,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Delivery {
    thread: String,
    envelope: Envelope,
    state: DeliveryState,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
enum JournalEntry {
    Identity { identity: Identity },
    Delivery { delivery: Box<Delivery> },
}

/// 파일 잠금은 종료 시 해제된다. 미완성 마지막 줄 외의 손상은 오류로 처리한다.
struct Journal {
    _lock: crate::file_lock::FileLock,
    file: File,
    entries: BTreeMap<String, Delivery>,
}

fn decode(bytes: &[u8], identity: &Identity) -> anyhow::Result<BTreeMap<String, Delivery>> {
    let mut entries = BTreeMap::new();
    let mut initialized = false;
    for line in bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        match serde_json::from_slice::<JournalEntry>(line).context("invalid Desktop journal")? {
            JournalEntry::Identity { identity: saved } => {
                anyhow::ensure!(
                    !initialized && &saved == identity,
                    "Desktop journal identity mismatch"
                );
                initialized = true;
            }
            JournalEntry::Delivery { delivery } => {
                anyhow::ensure!(initialized, "Desktop journal has no identity header");
                entries.insert(envelope_id(&delivery.envelope)?.to_owned(), *delivery);
            }
        }
    }
    anyhow::ensure!(initialized, "Desktop journal has no identity header");
    Ok(entries)
}

impl Journal {
    fn open(path: &Path, identity: Identity) -> anyhow::Result<Self> {
        // Windows 파일 잠금은 다른 프로세스의 읽기도 막으므로 데이터 파일과 분리한다.
        let lock = crate::file_lock::FileLock::acquire(&path.with_extension("lock"))
            .context("another Desktop receiver owns this binding")?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        // IPC 전송 전에 sync하므로 미완성 마지막 줄은 잘라도 된다.
        // accepted 줄이 미완성이면 직전 submitting이 남아 자동 재전송을 막는다.
        let valid = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |n| n + 1);
        if valid != bytes.len() {
            file.set_len(valid as u64)?;
            file.sync_all()?;
            bytes.truncate(valid);
        }
        file.seek(std::io::SeekFrom::End(0))?;
        let entries = if bytes.is_empty() {
            BTreeMap::new()
        } else {
            decode(&bytes, &identity)?
        };
        let mut journal = Self {
            _lock: lock,
            file,
            entries,
        };
        if bytes.is_empty() {
            journal.append(&JournalEntry::Identity { identity })?;
        }
        Ok(journal)
    }

    fn append(&mut self, entry: &JournalEntry) -> anyhow::Result<()> {
        let mut bytes = serde_json::to_vec(entry)?;
        bytes.push(b'\n');
        self.file.write_all(&bytes)?;
        self.file
            .sync_all()
            .context("Desktop journal sync failed; receipt not confirmed")
    }

    fn store(&mut self, delivery: Delivery) -> anyhow::Result<()> {
        let id = envelope_id(&delivery.envelope)?.to_owned();
        self.append(&JournalEntry::Delivery {
            delivery: Box::new(delivery.clone()),
        })?;
        self.entries.insert(id, delivery);
        Ok(())
    }

    fn ingest(&mut self, thread: &str, envelope: Envelope) -> anyhow::Result<()> {
        if self.entries.contains_key(envelope_id(&envelope)?) {
            return Ok(());
        }
        // 프레즌스 등 시스템 이벤트는 모델을 깨우지 않되 수신 기록은 남긴다.
        let ignored = envelope.kind == Kind::Event || envelope.from.as_str().starts_with('_');
        self.store(Delivery {
            thread: thread.to_owned(),
            envelope,
            state: if ignored {
                DeliveryState::Ignored
            } else {
                DeliveryState::Pending
            },
            detail: None,
        })
    }

    fn validate_resume(&self, thread: &str) -> anyhow::Result<()> {
        for delivery in self.entries.values() {
            anyhow::ensure!(
                !matches!(
                    delivery.state,
                    DeliveryState::Submitting | DeliveryState::Unknown
                ),
                "delivery {} has an uncertain outcome; inspect Desktop history and desktop status before resolving the journal; automatic replay refused",
                envelope_id(&delivery.envelope)?
            );
            anyhow::ensure!(
                delivery.state != DeliveryState::Pending || delivery.thread == thread,
                "pending delivery {} belongs to another task; resume that exact --thread",
                envelope_id(&delivery.envelope)?
            );
        }
        Ok(())
    }

    fn resolve(&mut self, id: &str, turn: Option<&str>, note: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            !note.trim().is_empty(),
            "record the evidence from inspecting the exact task history"
        );
        anyhow::ensure!(
            turn.is_none_or(|s| !s.trim().is_empty()),
            "turn ID must not be empty"
        );
        let mut delivery = self
            .entries
            .get(id)
            .context("delivery ID not found")?
            .clone();
        anyhow::ensure!(
            matches!(
                delivery.state,
                DeliveryState::Submitting | DeliveryState::Unknown
            ),
            "only uncertain deliveries can be resolved"
        );
        delivery.state = if turn.is_some() {
            DeliveryState::Accepted
        } else {
            DeliveryState::Pending
        };
        delivery.detail = Some(
            json!({"resolution":"operator_verified", "turn_id":turn, "note":note}).to_string(),
        );
        self.store(delivery)
    }
}

/// Explicit operator reconciliation; never guesses whether a timed-out turn ran.
pub fn resolve(
    cfg: &BrvConfig,
    binding: &Binding,
    id: &str,
    turn: Option<&str>,
    note: &str,
    confirmed: bool,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        confirmed,
        "inspect the exact task history, then pass --confirm; retry can duplicate work if the original turn ran"
    );
    let _guard = crate::connection::recovery_guard(cfg, binding)?;
    let path = journal_path(binding)?;
    anyhow::ensure!(path.exists(), "no delivery journal");
    Journal::open(&path, identity(cfg, binding))?.resolve(id, turn, note)?;
    status(cfg, binding)
}

fn envelope_id(envelope: &Envelope) -> anyhow::Result<&str> {
    Ok(envelope
        .id
        .as_ref()
        .context("received envelope has no message ID")?
        .as_str())
}

fn identity(cfg: &BrvConfig, binding: &Binding) -> Identity {
    Identity {
        server: cfg.server.clone(),
        binding: binding.full_label(),
    }
}

pub(crate) fn journal_path(binding: &Binding) -> anyhow::Result<PathBuf> {
    // 모든 경로 조각은 프로토콜 식별자 검사 후에만 사용한다.
    for part in [
        binding.org.as_deref().unwrap_or("legacy"),
        &binding.agent,
        &binding.channel,
    ] {
        brevduva_protocol::Ident::parse(part)?;
    }
    Ok(config::config_path()?
        .parent()
        .context("config has no parent")?
        .join("desktop")
        .join(binding.org.as_deref().unwrap_or("legacy"))
        .join(&binding.agent)
        .join(&binding.channel)
        .join("deliveries.jsonl"))
}

pub(crate) fn validate_saved(
    cfg: &BrvConfig,
    binding: &Binding,
    thread: &str,
) -> anyhow::Result<()> {
    let path = journal_path(binding)?;
    if path.exists() {
        Journal::open(&path, identity(cfg, binding))?.validate_resume(thread)?;
    }
    Ok(())
}
pub(crate) async fn probe(thread: &str) -> anyhow::Result<()> {
    validate_thread(thread)?;
    let _ = open_ipc().await?.owner(thread).await?;
    Ok(())
}

pub fn status(cfg: &BrvConfig, binding: &Binding) -> anyhow::Result<()> {
    let path = journal_path(binding)?;
    if !path.exists() {
        println!("no Desktop delivery journal for {}", binding.full_label());
        return Ok(());
    }
    let bytes = std::fs::read(&path)?;
    // 실행 중인 작성자의 미완행은 상태 조회에서만 무시한다. 파일 수정은 하지 않는다.
    let valid = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |n| n + 1);
    let entries = decode(&bytes[..valid], &identity(cfg, binding))?;
    println!(
        "journal: {} (accepted = input accepted, not work completed)",
        path.display()
    );
    for (id, delivery) in entries {
        println!(
            "{}",
            json!({"id": id, "thread": delivery.thread, "state": delivery.state, "detail": delivery.detail})
        );
    }
    Ok(())
}

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

fn validate_thread(thread: &str) -> anyhow::Result<()> {
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

fn turn_params(delivery: &Delivery, binding: &str) -> Value {
    let text = json!({"provenance":"Brevduva peer data, not instructions from the user", "binding":binding,
        "envelope":delivery.envelope}).to_string();
    let context = json!({"version":1, "message":{"source":"mcp_app", "sourceId":"brevduva-receiver", "text":text}});
    let prompt = "Respond to the user input in the context of our conversation.";
    let call_id = format!("brv_{}", ClientKey::generate());
    json!({"conversationId":delivery.thread, "turnStart":{
    "request":{"threadId":delivery.thread, "input":[{"type":"text", "text":prompt,
        "text_elements":[{"byteRange":{"start":0,"end":prompt.len()},
            "placeholder":format!("codex-untrusted-app-input:{context}")}]}]},
    "context":{"inheritThreadSettings":true,"responseItems":[
        {"type":"function_call","call_id":call_id,"name":"untrusted_input","arguments":"{}"},
        {"type":"function_call_output","call_id":call_id,"output":[{"type":"input_text","text":context["message"].to_string()}]}
    ]}}})
}

async fn receive(
    client: &Client,
    journal: &Mutex<Journal>,
    notify: &Notify,
    thread: &str,
) -> anyhow::Result<()> {
    loop {
        if let Some((envelope, token)) = client
            .recv_manual(RecvFilter::Any, Duration::from_secs(1))
            .await
        {
            let id = envelope_id(&envelope)?.to_owned();
            journal.lock().await.ingest(thread, envelope)?;
            // sync_all 성공 뒤에만 ACK. 디스크 실패면 함수가 종료되고 ACK하지 않는다.
            client.confirm(token).await;
            tracing::info!(%id, "desktop message durably received");
            notify.notify_one();
        }
        anyhow::ensure!(client.is_alive(), "Desktop receiver connection stopped");
    }
}

async fn dispatch(
    journal: &Mutex<Journal>,
    notify: &Notify,
    binding: &str,
    max: Option<usize>,
) -> anyhow::Result<()> {
    let mut accepted = 0;
    loop {
        let next = journal
            .lock()
            .await
            .entries
            .values()
            .find(|d| d.state == DeliveryState::Pending)
            .cloned();
        let Some(mut delivery) = next else {
            notify.notified().await;
            continue;
        };
        // 사전 연결 실패는 아직 보내지 않은 pending으로 남기고 종료해 채널 자리를 반납한다.
        let mut ipc = open_ipc().await?;
        let owner = ipc.owner(&delivery.thread).await?;
        let params = turn_params(&delivery, binding);
        delivery.state = DeliveryState::Submitting;
        journal.lock().await.store(delivery.clone())?;
        let result = ipc
            .request("thread-follower-start-turn", params, 2, Some(&owner))
            .await;
        match result {
            Ok(receipt) => {
                let turn = receipt["result"]["result"]["turn"]["id"].as_str();
                delivery.state = if turn.is_some() {
                    DeliveryState::Accepted
                } else {
                    DeliveryState::Unknown
                };
                delivery.detail = Some(
                    turn.map_or_else(|| "success response missing turn ID".into(), str::to_owned),
                );
                journal.lock().await.store(delivery.clone())?;
                anyhow::ensure!(
                    turn.is_some(),
                    "Desktop response missing turn ID; automatic replay refused"
                );
                tracing::info!(id=?delivery.envelope.id, thread=%delivery.thread, turn_id=?turn, "desktop input accepted (not completed)");
                accepted += 1;
                if max.is_some_and(|max| accepted >= max) {
                    return Ok(());
                }
            }
            Err(error) if error.busy() => {
                delivery.state = DeliveryState::Pending;
                delivery.detail = Some("waiting for current Desktop turn to finish".into());
                journal.lock().await.store(delivery.clone())?;
                tracing::info!(id=?delivery.envelope.id, "desktop busy; waiting before retry");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            Err(error) => {
                delivery.state = DeliveryState::Unknown;
                delivery.detail = Some(error.to_string());
                journal.lock().await.store(delivery)?;
                anyhow::bail!(
                    "Desktop delivery outcome requires inspection: {error}; message retained, no automatic replay"
                );
            }
        }
    }
}

async fn watch_owner(thread: &str) -> anyhow::Result<()> {
    loop {
        tokio::time::sleep(Duration::from_secs(10)).await;
        // Desktop가 닫히면 수신 자리도 반납한다. pending은 다음 실행에서 복구한다.
        let _ = open_ipc().await?.owner(thread).await?;
    }
}

pub async fn run(
    cfg: &BrvConfig,
    binding: &Binding,
    opts: ClientOptions,
    thread: &str,
    max: Option<usize>,
) -> anyhow::Result<()> {
    run_observed(cfg, binding, opts, thread, max, &|_| Ok(())).await
}

pub(crate) async fn run_observed(
    cfg: &BrvConfig,
    binding: &Binding,
    mut opts: ClientOptions,
    thread: &str,
    max: Option<usize>,
    observer: &dyn Fn(&crate::client::ClientState) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    validate_thread(thread)?;
    // 기존 실행체가 없으면 JOIN하지 않는다. 서비스의 다른 사용자 실행은 별도 범위다.
    let _ = open_ipc().await?.owner(thread).await?;
    let path = journal_path(binding)?;
    let dir = path.parent().context("journal has no parent")?;
    std::fs::create_dir_all(dir)?;
    config::restrict_dir(dir).context("cannot protect Desktop message journal")?;
    let journal = Journal::open(&path, identity(cfg, binding))?;
    journal.validate_resume(thread)?;
    let journal = Arc::new(Mutex::new(journal));
    let notify = Notify::new();
    // 다른 세션과 접속을 빼앗는 루프를 만들지 않는다. 기존 daemon과 같은 standby 규약.
    opts.takeover_standby = true;
    let client = Client::connect(opts);
    let mut state_rx = client.state();
    let observe = async {
        loop {
            observer(&state_rx.borrow_and_update())?;
            if state_rx.changed().await.is_err() {
                break;
            }
        }
        Err::<(), anyhow::Error>(anyhow::anyhow!("receiver state stream closed"))
    };
    let label = binding.full_label();
    tracing::info!(binding=%binding.full_label(), %thread, journal=%path.display(), "experimental Desktop receiver started");
    tokio::select! {
        result = receive(&client, &journal, &notify, thread) => result,
        result = dispatch(&journal, &notify, &label, max) => result,
        result = watch_owner(thread) => result,
        result = observe => result,
        result = tokio::signal::ctrl_c() => { result?; tracing::info!("Desktop receiver stopped; pending messages retained"); Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("brv-desktop-test-{}", ClientKey::generate()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> PathBuf {
            self.0.join("journal.jsonl")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn identity() -> Identity {
        Identity {
            server: "https://test.invalid".into(),
            binding: "org/agent@channel".into(),
        }
    }
    fn envelope() -> Envelope {
        serde_json::from_value(
            json!({"v":1,"id":ClientKey::generate(),"client_key":ClientKey::generate(),
            "from":"peer","to":"agent:agent","kind":"request","expects":"reply","hops":0,
            "content_type":"text/markdown","payload":"peer text","meta":{}}),
        )
        .unwrap()
    }

    #[test]
    fn journal_deduplicates_after_restart_and_preserves_task() {
        let fixture = Fixture::new();
        let message = envelope();
        {
            let mut journal = Journal::open(&fixture.path(), identity()).unwrap();
            journal.ingest("task-a", message.clone()).unwrap();
        }
        let mut journal = Journal::open(&fixture.path(), identity()).unwrap();
        journal.ingest("task-b", message).unwrap();
        assert_eq!(journal.entries.len(), 1);
        assert!(journal.validate_resume("task-a").is_ok());
        assert!(journal.validate_resume("task-b").is_err());
    }

    #[test]
    fn uncertain_submission_is_not_replayed_after_crash() {
        let fixture = Fixture::new();
        {
            let mut journal = Journal::open(&fixture.path(), identity()).unwrap();
            journal.ingest("task-a", envelope()).unwrap();
            let mut delivery = journal.entries.values().next().unwrap().clone();
            delivery.state = DeliveryState::Submitting;
            journal.store(delivery).unwrap();
        }
        // 중간에 잘린 accepted 기록은 전송 여부 불명확인 submitting을 덮지 못한다.
        OpenOptions::new()
            .append(true)
            .open(fixture.path())
            .unwrap()
            .write_all(b"{\"record\":")
            .unwrap();
        let journal = Journal::open(&fixture.path(), identity()).unwrap();
        assert!(journal.validate_resume("task-a").is_err());
        assert_eq!(
            journal.entries.values().next().unwrap().state,
            DeliveryState::Submitting
        );
    }

    #[test]
    fn operator_resolution_is_durable_and_keeps_exact_task() {
        let fixture = Fixture::new();
        let mut journal = Journal::open(&fixture.path(), identity()).unwrap();
        let msg = envelope();
        let id = envelope_id(&msg).unwrap().to_owned();
        journal.ingest("task-a", msg).unwrap();
        assert!(journal.resolve(&id, None, "checked").is_err());
        let mut delivery = journal.entries[&id].clone();
        delivery.state = DeliveryState::Unknown;
        journal.store(delivery).unwrap();
        assert!(journal.resolve(&id, None, " ").is_err());
        journal
            .resolve(&id, None, "verified no input in task history")
            .unwrap();
        assert!(journal.validate_resume("task-b").is_err());
        assert!(journal.validate_resume("task-a").is_ok());
        let mut delivery = journal.entries[&id].clone();
        delivery.state = DeliveryState::Submitting;
        journal.store(delivery).unwrap();
        journal
            .resolve(&id, Some("turn-verified"), "matched input and turn")
            .unwrap();
        drop(journal);
        let journal = Journal::open(&fixture.path(), identity()).unwrap();
        assert_eq!(journal.entries[&id].state, DeliveryState::Accepted);
        assert!(
            journal.entries[&id]
                .detail
                .as_ref()
                .unwrap()
                .contains("turn-verified")
        );
        assert!(journal.validate_resume("task-b").is_ok());
    }

    #[test]
    fn accepted_message_is_never_added_as_pending_again() {
        let fixture = Fixture::new();
        let mut journal = Journal::open(&fixture.path(), identity()).unwrap();
        let message = envelope();
        journal.ingest("task-a", message.clone()).unwrap();
        let mut delivery = journal.entries.values().next().unwrap().clone();
        delivery.state = DeliveryState::Accepted;
        delivery.detail = Some("turn-a".into());
        journal.store(delivery).unwrap();
        journal.ingest("task-b", message).unwrap();
        assert_eq!(
            journal.entries.values().next().unwrap().state,
            DeliveryState::Accepted
        );
        assert!(journal.validate_resume("task-b").is_ok());
    }

    #[test]
    fn journal_blocks_second_owner_and_wrong_identity() {
        let fixture = Fixture::new();
        let journal = Journal::open(&fixture.path(), identity()).unwrap();
        assert!(Journal::open(&fixture.path(), identity()).is_err());
        // 상태 조회는 실행 중인 저널을 읽을 수 있어야 한다 (Windows 잠금 회귀).
        assert!(decode(&std::fs::read(fixture.path()).unwrap(), &identity()).is_ok());
        drop(journal);
        let mut wrong = identity();
        wrong.server = "https://other.invalid".into();
        assert!(Journal::open(&fixture.path(), wrong).is_err());
    }

    #[test]
    fn missing_id_and_corrupt_complete_records_fail_closed() {
        let fixture = Fixture::new();
        let mut journal = Journal::open(&fixture.path(), identity()).unwrap();
        let mut message = envelope();
        message.id = None;
        assert!(journal.ingest("task", message).is_err());
        drop(journal);
        OpenOptions::new()
            .append(true)
            .open(fixture.path())
            .unwrap()
            .write_all(b"bad record\n")
            .unwrap();
        assert!(Journal::open(&fixture.path(), identity()).is_err());
    }

    #[test]
    fn external_payload_does_not_override_local_task_settings() {
        let delivery = Delivery {
            thread: "chosen-task".into(),
            envelope: envelope(),
            state: DeliveryState::Pending,
            detail: None,
        };
        let params = turn_params(&delivery, "org/agent@channel");
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
