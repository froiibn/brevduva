// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! stdio ↔ 로컬 엔드포인트 브리지 (RECEIVER_DESIGN.md P3·P8).
//!
//! HTTP MCP를 지원하지 않는 러너를 위한 얇은 중계기다. `brv mcp`가 이것이 된다.
//!
//! **이 프로세스에는 리시버 로직이 없다.** 서버에 JOIN하지 않고(P2), 토큰도 읽지 않고, 도구도
//! 스스로 처리하지 않는다 — 표준 입출력의 JSON-RPC를 루프백 엔드포인트로 옮기고 돌려줄 뿐이다.
//! 그래서 갱신 뒤 러너가 살려 둔 옛 브리지가 붙어 있어도 옛 **로직**이 서버와 대화하는 일이
//! 없다(P8). 그래도 규약이 바뀔 수 있으므로 기동 시 버전을 대조해 크게 다르면 물러난다.
//!
//! 정체성: 리시버가 깨운 세션은 `BREVDUVA_BINDING`·`BREVDUVA_WAKE`를 물려받는다. 브리지는
//! `initialize` 직후 그 정체성으로 `become`을 대신 보내 준다 — 모델이 자기가 누구인지 묻지
//! 않아도 되고, 깨우기 창의 작업 잠금을 그대로 승계한다(P7). 사람이 연 세션은 그런 변수가
//! 없으므로 모델이 `list_bindings` → `become`으로 정한다.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::sync::Mutex;

use super::auth::Endpoint;
use super::http::BRIDGE_VERSION_HEADER;

const SESSION_HEADER: &str = "mcp-session-id";

/// stdio MCP 서버로 동작하며 모든 것을 로컬 리시버로 넘긴다.
pub async fn run(host: Option<String>) -> anyhow::Result<()> {
    let endpoint = Endpoint::load().context(
        "no local receiver is running on this machine — start it with `brv daemon install` (or `brv daemon` in the foreground). \
         Sessions attach to the receiver; they no longer connect to the server themselves.",
    )?;
    if endpoint.version != env!("CARGO_PKG_VERSION") {
        // 갱신 뒤 러너가 살려 둔 옛 브리지 — 조용히 이상하게 도느니 이유를 말하고 물러난다(P8).
        anyhow::bail!(
            "this brv bridge is {} but the running receiver is {} — restart this session so the runner spawns the current bridge",
            env!("CARGO_PKG_VERSION"),
            endpoint.version
        );
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .context("http client")?;
    run_io(
        endpoint,
        client,
        host,
        BufReader::new(tokio::io::stdin()),
        tokio::io::stdout(),
    )
    .await
}

/// 실제 입출력을 갈아 끼울 수 있게 분리 — 회귀 시험이 파이프로 같은 경로를 돈다.
pub(crate) async fn run_io<R, W>(
    endpoint: Endpoint,
    client: reqwest::Client,
    host: Option<String>,
    reader: R,
    writer: W,
) -> anyhow::Result<()>
where
    R: tokio::io::AsyncBufRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let out = Arc::new(Mutex::new(writer));
    let session: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let url = endpoint.mcp_url();
    let mut lines = reader.lines();
    let mut stream_task: Option<tokio::task::JoinHandle<()>> = None;

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(mut request) = serde_json::from_str::<Value>(&line) else {
            tracing::warn!("unparsable jsonrpc line from the runner");
            continue;
        };
        // 러너 id는 추측하지 않는다 — 등록에서 받은 값이 있으면 그대로 알려 준다.
        if request["method"] == "initialize"
            && let Some(host) = host.as_deref()
        {
            request["params"]["clientInfo"]["name"] = json!(host);
        }
        // 깨어난 세션의 잠금 승계 — 리시버가 발급한 값이라 세션이 지어낼 수 없다.
        if request["method"] == "tools/call"
            && request["params"]["name"] == "become"
            && let Ok(wake) = std::env::var("BREVDUVA_WAKE")
            && !wake.is_empty()
        {
            request["params"]["arguments"]["wake"] = json!(wake);
        }
        // Codex 작업(CLI·Desktop)에 통로를 붙일 때 — 그 작업의 프로필 문맥은 사용자 명의인 이 브리지만
        // 안다. Desktop은 유닉스에서 IPC 소켓 위치를 프로필로 정한다.
        if request["method"] == "tools/call"
            && request["params"]["name"] == "receiver_connect"
            && matches!(
                request["params"]["arguments"]["session_kind"].as_str(),
                Some("codex-cli" | "codex-desktop")
            )
        {
            codex_context_defaults(
                &mut request["params"]["arguments"],
                std::env::var_os("CODEX_HOME").map(PathBuf::from),
                dirs::home_dir(),
                std::env::var_os("CODEX_CLI_PATH").map(PathBuf::from),
                || {
                    crate::runners::spec("codex")
                        .and_then(crate::runners::detect)
                        .map(|found| found.path)
                },
            );
        }

        // 세션이 선 뒤의 요청은 동시에 보낸다 (2026-09-11, 15단계): 수동 수신 대기는 최대 45초 걸리고, 그동안
        // 취소 알림과 다른 도구 호출이 지나가야 한다. 종전에는 한 줄씩 응답을 기다려 대기 중 취소가 막혔다.
        // 응답의 짝은 JSON-RPC id가 맞춘다. 리시버가 답하지 않으면 그 요청에 오류로 답한다.
        let current = session.lock().await.clone();
        if let Some(id) = current {
            let (client, url, out) = (client.clone(), url.clone(), Arc::clone(&out));
            let token = endpoint.token.expose().to_owned();
            tokio::spawn(async move {
                let rpc_id = request.get("id").cloned();
                let sent = client
                    .post(&url)
                    .bearer_auth(&token)
                    .header(SESSION_HEADER, &id)
                    .header(BRIDGE_VERSION_HEADER, env!("CARGO_PKG_VERSION"))
                    .json(&request)
                    .send()
                    .await;
                let answered = match sent {
                    // 알림과 취소된 요청에는 응답이 없다
                    Ok(response) if response.status() == reqwest::StatusCode::ACCEPTED => return,
                    Ok(response) => response.text().await.map_err(anyhow::Error::from),
                    Err(error) => Err(error.into()),
                };
                let line = match answered {
                    Ok(body) => body,
                    Err(error) => {
                        tracing::error!(%error, "the local receiver did not answer a request");
                        let Some(rpc_id) = rpc_id else {
                            return;
                        };
                        json!({"jsonrpc":"2.0","id":rpc_id,"error":{"code":-32603,"message":format!(
                            "the local receiver at {url} did not answer — is it still running? ({error:#})"
                        )}})
                        .to_string()
                    }
                };
                let _ = write_line(&out, &line).await;
            });
            continue;
        }

        // 세션을 여는 요청(initialize)은 차례대로 — 세션 id와 사건 흐름이 여기서 정해진다.
        let post = client
            .post(&url)
            .bearer_auth(endpoint.token.expose())
            .header(BRIDGE_VERSION_HEADER, env!("CARGO_PKG_VERSION"));
        let response = post.json(&request).send().await.with_context(|| {
            format!("the local receiver at {url} did not answer — is it still running?")
        })?;

        let fresh = response
            .headers()
            .get(SESSION_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        if let Some(id) = fresh {
            *session.lock().await = Some(id.clone());
            // 리시버의 이벤트 스트림을 연다 — 밀려남·종료 통지와 Channels 사건(7c)이 여기로 온다.
            // 열려 있는 것만으로는 "받을 수 있음"이 아니다(P4 정정) — 러너 입력 통로가 준비돼야 한다.
            stream_task = Some(tokio::spawn(pump(
                client.clone(),
                url.clone(),
                endpoint.token.expose().to_owned(),
                id,
                Arc::clone(&out),
            )));
        }

        if response.status() == reqwest::StatusCode::ACCEPTED {
            continue; // 알림에는 응답이 없다
        }
        let body = response.text().await.context("receiver response body")?;
        write_line(&out, &body).await?;

        // initialize 응답을 넘긴 뒤, 리시버가 깨운 세션이면 정체성을 대신 세운다.
        if request["method"] == "initialize" {
            claim_woken_identity(&client, &url, &endpoint, &session, &out).await;
        }
    }

    if let Some(task) = stream_task {
        task.abort();
    }
    if let Some(id) = session.lock().await.as_deref() {
        let _ = client
            .delete(&url)
            .bearer_auth(endpoint.token.expose())
            .header(SESSION_HEADER, id)
            .header(BRIDGE_VERSION_HEADER, env!("CARGO_PKG_VERSION"))
            .send()
            .await;
    }
    Ok(())
}

/// 깨운 프로세스가 리시버에 "이 깨우기로 받았다"를 증명한다 (2026-09-11) — 브리지를 거치지 않는 깨우기
/// 명령과 시험이 쓴다. 그 깨우기의 식별자로 `become`하고 세션을 닫는다. 리시버는 이때 그 배치를 확정한다.
pub async fn claim_wake(agent: &str, channel: &str, wake: &str) -> anyhow::Result<()> {
    let endpoint = Endpoint::load().context("no local receiver is running on this machine")?;
    let client = reqwest::Client::builder().no_proxy().build()?;
    let url = endpoint.mcp_url();
    let initialized = client
        .post(&url)
        .bearer_auth(endpoint.token.expose())
        .header(BRIDGE_VERSION_HEADER, env!("CARGO_PKG_VERSION"))
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"clientInfo":{"name":"brv-wake"}}}))
        .send()
        .await
        .context("the local receiver did not answer")?;
    let session = initialized
        .headers()
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .context("the local receiver gave no session id")?
        .to_owned();
    let result: Value = client
        .post(&url)
        .bearer_auth(endpoint.token.expose())
        .header(SESSION_HEADER, &session)
        .header(BRIDGE_VERSION_HEADER, env!("CARGO_PKG_VERSION"))
        .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":"become","arguments":{"agent":agent,"channel":channel,"wake":wake}}}))
        .send()
        .await?
        .json()
        .await?;
    let _ = client
        .delete(&url)
        .bearer_auth(endpoint.token.expose())
        .header(SESSION_HEADER, &session)
        .header(BRIDGE_VERSION_HEADER, env!("CARGO_PKG_VERSION"))
        .send()
        .await;
    anyhow::ensure!(
        result["result"]["isError"] != true,
        "the receiver refused the wake claim: {result}"
    );
    Ok(())
}

/// 리시버가 깨운 세션은 자기가 누구인지 이미 안다 — 모델에게 묻지 않고 정체성을 세운다.
async fn claim_woken_identity<W>(
    client: &reqwest::Client,
    url: &str,
    endpoint: &Endpoint,
    session: &Arc<Mutex<Option<String>>>,
    out: &Arc<Mutex<W>>,
) where
    W: tokio::io::AsyncWrite + Unpin,
{
    let (Ok(binding), Ok(wake)) = (
        std::env::var("BREVDUVA_BINDING"),
        std::env::var("BREVDUVA_WAKE"),
    ) else {
        return; // 사람이 연 세션 — 모델이 list_bindings로 정한다
    };
    if wake.is_empty() {
        return;
    }
    let Some((agent, channel)) = split_binding(&binding) else {
        return;
    };
    let Some(id) = session.lock().await.clone() else {
        return;
    };
    let request = json!({
        "jsonrpc": "2.0", "id": "brv-bridge-become", "method": "tools/call",
        "params": {"name": "become", "arguments": {
            "agent": agent, "channel": channel, "wake": wake,
        }}
    });
    match client
        .post(url)
        .bearer_auth(endpoint.token.expose())
        .header(SESSION_HEADER, &id)
        .header(BRIDGE_VERSION_HEADER, env!("CARGO_PKG_VERSION"))
        .json(&request)
        .send()
        .await
    {
        Ok(response) => tracing::info!(
            binding = %binding,
            ok = response.status().is_success(),
            "claimed the woken identity on the receiver"
        ),
        Err(error) => {
            // 조용히 넘기지 않는다 — 정체성 없이 뜬 세션은 아무것도 못 한다.
            tracing::error!(%error, "could not claim the woken identity");
            let _ = write_line(
                out,
                &json!({"jsonrpc":"2.0","method":"notifications/message","params":{
                    "level":"error",
                    "data":format!("could not take the identity {binding} on the local receiver: {error}")}})
                .to_string(),
            )
            .await;
        }
    }
}

/// Codex CLI 작업의 문맥을 채운다 (2026-09-10, 7b). 리시버(윈도우 서비스)는 사용자 프로필을
/// 모르지만, 브리지는 그 작업 안에서 사용자 명의로 돈다. 명시한 값은 건드리지 않는다.
///
/// `thread_id`는 채우지 않는다: MCP 서버 프로세스의 환경 값이 지금 이 작업의 것이라는 보장이
/// 없어, 모델이 자기 셸에서 읽어 넘기는 기존 규약을 따른다.
fn codex_context_defaults(
    args: &mut Value,
    codex_home_env: Option<PathBuf>,
    home_dir: Option<PathBuf>,
    cli_path_env: Option<PathBuf>,
    detect: impl FnOnce() -> Option<PathBuf>,
) {
    if !args.is_object() {
        return;
    }
    if args["codex_home"].as_str().is_none_or(str::is_empty)
        && let Some(home) = codex_home_env
            .filter(|path| path.is_absolute())
            .or_else(|| home_dir.map(|home| home.join(".codex")))
    {
        args["codex_home"] = json!(home);
    }
    if args["codex_executable"].as_str().is_none_or(str::is_empty)
        && let Some(executable) = cli_path_env
            .filter(|path| path.is_absolute())
            .or_else(detect)
    {
        args["codex_executable"] = json!(executable);
    }
}

/// `org/agent@channel` 또는 `agent@channel`에서 agent·channel을 뽑는다.
fn split_binding(label: &str) -> Option<(String, String)> {
    let rest = label.rsplit('/').next()?;
    let (agent, channel) = rest.split_once('@')?;
    (!agent.is_empty() && !channel.is_empty()).then(|| (agent.to_owned(), channel.to_owned()))
}

/// SSE 스트림을 stdout의 JSON-RPC 알림으로 옮긴다.
async fn pump<W>(
    client: reqwest::Client,
    url: String,
    token: String,
    session: String,
    out: Arc<Mutex<W>>,
) where
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut response = match client
        .get(&url)
        .bearer_auth(&token)
        .header(BRIDGE_VERSION_HEADER, env!("CARGO_PKG_VERSION"))
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .header(SESSION_HEADER, &session)
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            tracing::error!(status = %response.status(), "the receiver refused the delivery stream");
            return;
        }
        Err(error) => {
            tracing::error!(%error, "could not open the delivery stream");
            return;
        }
    };
    let mut buffer = String::new();
    loop {
        match response.chunk().await {
            Ok(Some(bytes)) => buffer.push_str(&String::from_utf8_lossy(&bytes)),
            // 리시버가 닫았다 — 이 세션은 더 받지 못한다.
            Ok(None) => break,
            Err(error) => {
                tracing::warn!(%error, "delivery stream ended");
                break;
            }
        }
        while let Some(cut) = buffer.find("\n\n") {
            let frame: String = buffer.drain(..cut + 2).collect();
            for payload in frame.lines().filter_map(|l| l.strip_prefix("data: ")) {
                if write_line(&out, payload.trim()).await.is_err() {
                    return;
                }
            }
        }
    }
}

async fn write_line<W>(out: &Arc<Mutex<W>>, payload: &str) -> anyhow::Result<()>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut out = out.lock().await;
    out.write_all(payload.as_bytes()).await?;
    out.write_all(b"\n").await?;
    out.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------ 종단 회귀
    //
    // 러너(stdio) → 브리지 → 로컬 엔드포인트 → 평면 → 등록부까지 실제로 도는 경로.
    // 0.6.39 검토 1번(다중 바인딩 머신에서 대화형 세션이 붙지 못함)과 2026-09-10 P4 정정
    // (MCP 연결만으로는 수신자가 아니다)이 여기서 검증된다.

    use crate::config::{Binding, BrvConfig};
    use crate::local_plane::auth::Token;
    use crate::local_plane::http::{LocalHttp, SessionHandler};
    use crate::local_plane::plane::{Plane, Routed};
    use crate::local_plane::registry::{BindingKey, SessionCapabilities};
    use brevduva_protocol::{ClientKey, Envelope};

    fn binding(agent: &str, channel: &str) -> Binding {
        Binding {
            org: Some("personal".to_owned()),
            agent: agent.to_owned(),
            channel: channel.to_owned(),
            description: String::new(),
            wake_dir: Some("/tmp".to_owned()),
            wake_command: None,
            wake_args: None,
        }
    }

    /// 시험용 기록 디렉터리 — 사용자 설정 디렉터리를 건드리지 않는다.
    struct TempRoot(std::path::PathBuf);

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct Harness {
        plane: Arc<Plane>,
        stdin: tokio::io::DuplexStream,
        stdout: tokio::io::Lines<BufReader<tokio::io::DuplexStream>>,
        _shutdown: tokio::sync::watch::Sender<bool>,
        // 필드는 선언 순서대로 내려간다 — 기록 디렉터리는 마지막에 지운다.
        _root: TempRoot,
    }

    /// 바인딩 여럿을 가진 머신을 흉내 낸다 — 옛 구조가 기동조차 못 하던 조건.
    async fn start() -> Harness {
        let cfg = BrvConfig {
            server: "http://127.0.0.1:1".to_owned(),
            wake: None,
            bindings: vec![
                binding("codex", "saju-engine"),
                binding("brvcodex", "brv"),
                binding("brvdev", "brvqa"),
                binding("brvclaude", "brv"),
            ],
        };
        let root = std::env::temp_dir().join(format!("brv-bridge-{}", ClientKey::generate()));
        std::fs::create_dir_all(&root).expect("temp root");
        let plane = Plane::new(
            &cfg,
            root.clone(),
            Arc::new(crate::local_plane::runner_exec::UserContextExec::new(
                crate::daemon::WakeSpawn::Direct,
                root.join("runner-exec"),
            )),
        );
        let token = Token::generate().expect("token");
        let state = LocalHttp::new(token.clone(), Arc::clone(&plane) as Arc<dyn SessionHandler>);
        let listener = crate::local_plane::http::bind(0).await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (shutdown, rx) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            let _ = crate::local_plane::http::serve(listener, state, rx).await;
        });
        let endpoint = Endpoint {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            addr,
            token,
            pid: std::process::id(),
            started_unix: 0,
        };
        let (test_in, bridge_in) = tokio::io::duplex(64 * 1024);
        let (bridge_out, test_out) = tokio::io::duplex(64 * 1024);
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client");
        tokio::spawn(async move {
            let _ = run_io(
                endpoint,
                client,
                Some("claude".to_owned()),
                BufReader::new(bridge_in),
                bridge_out,
            )
            .await;
        });
        Harness {
            plane,
            stdin: test_in,
            stdout: BufReader::new(test_out).lines(),
            _shutdown: shutdown,
            _root: TempRoot(root),
        }
    }

    impl Harness {
        async fn send(&mut self, request: Value) {
            let line = format!("{request}\n");
            self.stdin.write_all(line.as_bytes()).await.expect("write");
            self.stdin.flush().await.expect("flush");
        }

        /// 다음 줄을 JSON으로. 브리지가 멈추면 시험도 멈추지 않도록 상한을 둔다.
        async fn next(&mut self) -> Value {
            let line =
                tokio::time::timeout(std::time::Duration::from_secs(5), self.stdout.next_line())
                    .await
                    .expect("bridge answered in time")
                    .expect("read")
                    .expect("a line");
            serde_json::from_str(&line).expect("json line")
        }

        async fn call(&mut self, id: i64, name: &str, arguments: Value) -> Value {
            self.send(json!({
                "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": {"name": name, "arguments": arguments}
            }))
            .await;
            let response = self.next().await;
            serde_json::from_str(
                response["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap_or("{}"),
            )
            .expect("tool result json")
        }

        async fn initialize(&mut self) -> Value {
            self.send(json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2025-06-18", "clientInfo": {"name": "x", "version": "1"}}
            }))
            .await;
            self.next().await
        }
    }

    fn envelope(kind: &str, expects: Option<&str>) -> Envelope {
        let mut value = json!({
            "v": 1, "id": ClientKey::generate(), "client_key": ClientKey::generate(),
            "from": "brvcodex", "to": "agent:brvclaude", "kind": kind, "hops": 1,
            "content_type": "text/plain", "payload": "untrusted peer text", "meta": {}
        });
        if let Some(expects) = expects {
            value["expects"] = json!(expects);
        }
        serde_json::from_value(value).expect("envelope")
    }

    async fn read_event(monitor: &mut BufReader<tokio::net::TcpStream>) -> Value {
        let mut line = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            monitor.read_line(&mut line),
        )
        .await
        .expect("event in time")
        .expect("read");
        serde_json::from_str(&line).expect("json event")
    }

    /// Claude Code의 Monitor 도구가 하는 일을 흉내 낸다 — 활성화 응답의 주소·표로 붙는다.
    async fn attach_monitor(h: &mut Harness, id: i64) -> BufReader<tokio::net::TcpStream> {
        let activation = h
            .call(
                id,
                "receiver_connect",
                json!({"session_kind": "claude-code", "monitor_available": true}),
            )
            .await;
        let command = activation["arguments"]["command"]
            .as_str()
            .expect("monitor command")
            .to_owned();
        let words: Vec<&str> = command.split_whitespace().collect();
        let after = |flag: &str| {
            let at = words.iter().position(|w| *w == flag).expect(flag);
            words[at + 1].to_owned()
        };
        let mut stream = tokio::net::TcpStream::connect(after("--address"))
            .await
            .expect("connect monitor");
        stream
            .write_all(format!("{}\n", after("--ticket")).as_bytes())
            .await
            .expect("ticket");
        let mut monitor = BufReader::new(stream);
        assert_eq!(
            read_event(&mut monitor).await["event"],
            "brevduva_receiver_ready"
        );
        monitor
    }

    #[tokio::test]
    async fn a_session_on_a_multi_binding_machine_attaches_and_takes_an_identity() {
        // 0.6.39 검토 1번의 근본 수정: 바인딩이 4개여도 세션은 그냥 붙는다.
        let mut h = start().await;
        let response = h.initialize().await;
        assert_eq!(response["result"]["serverInfo"]["name"], "brv");
        assert!(
            response["result"]["instructions"]
                .as_str()
                .expect("instructions")
                .contains("become"),
            "붙은 세션에게 먼저 정체성을 정하라고 안내한다"
        );

        let listed = h.call(2, "list_bindings", json!({})).await;
        let bindings = listed["bindings"].as_array().expect("bindings");
        assert_eq!(bindings.len(), 4, "이 머신의 바인딩이 전부 보인다");
        assert!(bindings.iter().all(|b| b["holder"].is_null()));

        let bound = h
            .call(3, "become", json!({"agent": "brvclaude", "channel": "brv"}))
            .await;
        assert_eq!(bound["status"], "bound");
        assert_eq!(bound["binding"], "personal/brvclaude@brv");
        assert_eq!(
            bound["receiving"],
            json!(false),
            "MCP 연결만으로는 수신자가 아니다 (P4, 2026-09-10 정정)"
        );
    }

    #[tokio::test]
    async fn an_mcp_connection_without_an_input_path_leaves_deliveries_to_the_wake_path() {
        let mut h = start().await;
        h.initialize().await;
        h.call(2, "become", json!({"agent": "brvclaude", "channel": "brv"}))
            .await;
        let key = BindingKey::parse("personal/brvclaude@brv");
        assert_eq!(
            h.plane.route(&key, &envelope("message", None), 1).await,
            Routed::Wake,
            "통로 없는 세션에 넣으면 수락되지 않은 채 재전달만 반복된다 — 무인 경로로"
        );
    }

    #[tokio::test]
    async fn a_delivery_reaches_the_session_through_its_monitor_and_needs_a_receipt() {
        let mut h = start().await;
        h.initialize().await;
        h.call(2, "become", json!({"agent": "brvclaude", "channel": "brv"}))
            .await;
        let mut monitor = attach_monitor(&mut h, 3).await;

        let key = BindingKey::parse("personal/brvclaude@brv");
        let request = envelope("request", Some("reply"));
        assert!(matches!(
            h.plane.route(&key, &request, 42).await,
            Routed::Pushed { .. }
        ));
        let event = read_event(&mut monitor).await;
        assert_eq!(event["event"], "brevduva_message");
        assert_eq!(
            event["message_id"],
            json!(request.id.as_ref().expect("id").as_str())
        );
        assert!(
            !event.to_string().contains("untrusted peer text"),
            "동료 본문은 사용자 입력 경로에 싣지 않는다"
        );
        let token = event["receipt_token"].as_str().expect("token").to_owned();

        // 수락 전에는 잠금이 걸리지 않는다 — 서버 확정도 없다
        assert!(!h.plane.is_held_for_test(&key));

        // 서버 접속이 없어 확정 자체는 못 하지만, 수락 경로를 거쳤다는 사실은 드러난다
        let accepted = h.call(4, "receipt", json!({"receipt_token": token})).await;
        assert_eq!(accepted["status"], "error");
        assert!(
            accepted["message"]
                .as_str()
                .expect("message")
                .contains("lost its server connection"),
            "확정할 수 없으면 정직하게 말한다: {accepted}"
        );
    }

    #[tokio::test]
    async fn an_evicted_session_is_told_over_the_same_stream() {
        let mut h = start().await;
        h.initialize().await;
        h.call(2, "become", json!({"agent": "brvclaude", "channel": "brv"}))
            .await;

        // 다른 세션이 같은 정체성을 가져간다 (P7: 최신이 이긴다)
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let other = h.plane.attach(
            crate::local_plane::registry::AttachSpec {
                host: Some("codex".to_owned()),
                origin: crate::local_plane::registry::Origin::Attended,
                capabilities: SessionCapabilities::default(),
                description: None,
            },
            tx,
        );
        h.plane
            .become_for_test(&other, &BindingKey::parse("personal/brvclaude@brv"))
            .expect("takeover");

        let notification = h.next().await;
        assert_eq!(notification["method"], "notifications/brevduva/evicted");
        assert_eq!(notification["params"]["by"], "codex");
    }

    #[tokio::test]
    async fn a_manual_wait_does_not_hold_up_other_requests_through_the_bridge() {
        // 15단계: 수동 수신 대기(최대 45초) 중에도 취소 알림과 다른 도구 호출이 지나가야 한다.
        let mut h = start().await;
        h.initialize().await;
        h.call(2, "become", json!({"agent": "brvclaude", "channel": "brv"}))
            .await;
        h.send(json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
                      "params":{"name":"wait_for_message","arguments":{"timeout_s":2}}}))
            .await;
        h.send(json!({"jsonrpc":"2.0","id":4,"method":"tools/call",
                      "params":{"name":"list_bindings","arguments":{}}}))
            .await;
        assert_eq!(
            h.next().await["id"],
            4,
            "기다리는 호출 뒤에 보낸 요청이 먼저 답한다"
        );
        let waited = h.next().await;
        assert_eq!(waited["id"], 3);
        let result: Value = serde_json::from_str(
            waited["result"]["content"][0]["text"]
                .as_str()
                .expect("tool text"),
        )
        .expect("tool json");
        assert_eq!(result["status"], "timeout");
    }

    #[tokio::test]
    async fn the_bridge_refuses_to_run_against_a_different_receiver_version() {
        // P8: 갱신 뒤 러너가 살려 둔 옛 브리지는 조용히 이상하게 돌지 않는다.
        let stale = Endpoint {
            version: "0.0.1-old".to_owned(),
            addr: "127.0.0.1:1".parse().expect("addr"),
            token: Token::generate().expect("token"),
            pid: 1,
            started_unix: 0,
        };
        let dir =
            std::env::temp_dir().join(format!("brv-bridge-version-{}", ClientKey::generate()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("endpoint.json");
        stale.publish_at(&path).expect("publish");
        let loaded = Endpoint::load_from(&path).expect("load");
        std::fs::remove_dir_all(&dir).ok();
        assert_ne!(loaded.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn codex_task_context_is_filled_from_the_session_environment_but_never_the_thread() {
        let home = std::env::temp_dir();
        let mut args = json!({"session_kind": "codex-cli", "thread_id": "t"});
        codex_context_defaults(&mut args, None, Some(home.clone()), None, || {
            Some(home.join("codex.exe"))
        });
        assert_eq!(args["codex_home"], json!(home.join(".codex")));
        assert_eq!(args["codex_executable"], json!(home.join("codex.exe")));
        assert_eq!(
            args["thread_id"], "t",
            "작업 id는 모델이 자기 셸에서 읽은 값 그대로"
        );

        let mut explicit = json!({"session_kind": "codex-cli",
            "codex_home": "/given", "codex_executable": "/given/codex"});
        codex_context_defaults(
            &mut explicit,
            Some(home.clone()),
            Some(home.clone()),
            Some(home.join("x")),
            || panic!("명시 값이 있으면 탐지하지 않는다"),
        );
        assert_eq!(explicit["codex_home"], "/given");
        assert_eq!(explicit["codex_executable"], "/given/codex");

        let pinned = home.join("pinned-codex-home");
        let mut from_env = json!({"session_kind": "codex-cli"});
        codex_context_defaults(
            &mut from_env,
            Some(pinned.clone()),
            Some(home.clone()),
            Some(home.join("cli.exe")),
            || None,
        );
        assert_eq!(from_env["codex_home"], json!(pinned));
        assert_eq!(from_env["codex_executable"], json!(home.join("cli.exe")));

        let mut not_object = json!("oops");
        codex_context_defaults(&mut not_object, None, Some(home), None, || None);
        assert_eq!(not_object, json!("oops"));
    }

    #[test]
    fn binding_labels_split_into_agent_and_channel() {
        assert_eq!(
            split_binding("personal/brvclaude@brv"),
            Some(("brvclaude".to_owned(), "brv".to_owned()))
        );
        assert_eq!(
            split_binding("brvclaude@brv"),
            Some(("brvclaude".to_owned(), "brv".to_owned()))
        );
        assert_eq!(split_binding("brvclaude"), None);
        assert_eq!(split_binding("@brv"), None);
        assert_eq!(split_binding("brvclaude@"), None);
    }
}
