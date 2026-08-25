//! WS 클라이언트 코어 — PROTOCOL.md 13장(장애·재연결)의 **참조 구현**.
//!
//! - 재접속: 지수 백오프 + full jitter, JOIN 멱등 (13.2)
//! - 발행: OK/ERR를 못 받은 PUB는 재연결 후 같은 client_key로 재발행 (13.3)
//! - 수신: 서버 발급 `id`로 중복 제거, 소비 시점에 ACK (at-least-once의 클라이언트 절반)
//! - 하트비트: PING 20s, 2회 무응답 ≈ 45s면 연결 폐기 (13.1)
//!
//! 구조: 단일 액터 태스크가 연결·큐·대기자를 소유하고, 공개 API는 mpsc 명령으로 대화한다.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

use anyhow::Context as _;
use brevduva_protocol::{
    Address, Capabilities, ClientFrame, ClientKey, ClientOp, Envelope, ErrBody, ErrorCode, Expects,
    Ident, Kind, MessageId, OkBody, PresenceEntry, ServerFrame, ServerOp,
};
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Map, Value};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, interval, timeout};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug, Clone)]
pub struct ClientOptions {
    /// 서버 베이스 URL (http/https) — ws(s)://…/v1/ws 로 유도.
    pub server: String,
    pub channel: String,
    pub agent: String,
    pub token: String,
    pub description: String,
    /// 테이크오버(`agent/session-conflict`)를 받으면 재접속 대신 standby로 —
    /// 데몬용 (2.2). 대화형 어댑터는 false(즉시 재접속 = 기존 세션 탈환).
    pub takeover_standby: bool,
    /// standby 중 자리 확인 주기 — long-poll PRESENCE 프로브 (세션·큐를 건드리지 않음).
    pub standby_probe: Duration,
}

impl ClientOptions {
    pub fn new(
        server: impl Into<String>,
        channel: impl Into<String>,
        agent: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        Self {
            server: server.into(),
            channel: channel.into(),
            agent: agent.into(),
            token: token.into(),
            description: String::new(),
            takeover_standby: false,
            standby_probe: Duration::from_secs(30),
        }
    }
}

/// 발행 명세 — 엔벨로프의 클라이언트 채움분 (id·ts는 서버 소관, 3장).
#[derive(Debug, Clone)]
pub struct PublishSpec {
    pub to: String,
    pub kind: Kind,
    pub payload: Option<String>,
    pub content_type: String,
    pub expects: Option<Expects>,
    pub correlation_id: Option<String>,
    pub ttl_ms: Option<u64>,
    pub hops: u32,
    pub meta: Map<String, Value>,
}

impl PublishSpec {
    pub fn message(to: impl Into<String>, payload: impl Into<String>) -> Self {
        Self {
            to: to.into(),
            kind: Kind::Message,
            payload: Some(payload.into()),
            content_type: "text/markdown".to_owned(),
            expects: None,
            correlation_id: None,
            ttl_ms: None,
            hops: 0,
            meta: Map::new(),
        }
    }
}

/// 수신 필터 — wait_for_message(전체)와 wait_for_reply(correlation)는 같은 큐의 다른 뷰 (9.5).
#[derive(Debug, Clone)]
pub enum RecvFilter {
    Any,
    Correlation(String),
}

impl RecvFilter {
    fn matches(&self, env: &Envelope) -> bool {
        match self {
            Self::Any => true,
            Self::Correlation(c) => env
                .correlation_id
                .as_ref()
                .is_some_and(|id| id.as_str() == c),
        }
    }
}

enum Cmd {
    Publish(
        Box<PublishSpec>,
        oneshot::Sender<Result<MessageId, ErrBody>>,
    ),
    Recv(RecvFilter, oneshot::Sender<Envelope>),
    Fetch {
        after_id: Option<String>,
        limit: Option<u32>,
        resp: oneshot::Sender<Result<Vec<Envelope>, String>>,
    },
    Presence(oneshot::Sender<Result<Vec<PresenceEntry>, String>>),
}

/// 재연결·큐·ACK를 감춘 클라이언트 핸들. clone 가능.
#[derive(Clone)]
pub struct Client {
    cmds: mpsc::Sender<Cmd>,
}

impl Client {
    pub fn connect(opts: ClientOptions) -> Self {
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(actor(opts, rx));
        Self { cmds: tx }
    }

    /// 발행 — 단절 중이면 재연결 후 같은 client_key로 재시도된다 (13.3).
    /// 호출자는 어댑터 정직성 규약(13.4)에 따라 자체 타임아웃을 걸어라.
    pub async fn publish(&self, spec: PublishSpec) -> Result<MessageId, ErrBody> {
        let (tx, rx) = oneshot::channel();
        if self
            .cmds
            .send(Cmd::Publish(Box::new(spec), tx))
            .await
            .is_err()
        {
            return Err(dead_client_err());
        }
        rx.await.unwrap_or_else(|_| Err(dead_client_err()))
    }

    /// 다음 메시지 수신 (소비 시 ACK). 타임아웃이면 None — 메시지는 큐에 남는다.
    pub async fn recv(&self, filter: RecvFilter, wait: Duration) -> Option<Envelope> {
        let (tx, rx) = oneshot::channel();
        self.cmds.send(Cmd::Recv(filter, tx)).await.ok()?;
        timeout(wait, rx).await.ok()?.ok()
    }

    pub async fn fetch(
        &self,
        after_id: Option<String>,
        limit: Option<u32>,
        wait: Duration,
    ) -> Result<Vec<Envelope>, String> {
        let (tx, rx) = oneshot::channel();
        self.cmds
            .send(Cmd::Fetch {
                after_id,
                limit,
                resp: tx,
            })
            .await
            .map_err(|_| "client stopped".to_owned())?;
        timeout(wait, rx)
            .await
            .map_err(|_| "fetch timed out; retry".to_owned())?
            .unwrap_or_else(|_| Err("connection lost; retry".to_owned()))
    }

    pub async fn presence(&self, wait: Duration) -> Result<Vec<PresenceEntry>, String> {
        let (tx, rx) = oneshot::channel();
        self.cmds
            .send(Cmd::Presence(tx))
            .await
            .map_err(|_| "client stopped".to_owned())?;
        timeout(wait, rx)
            .await
            .map_err(|_| "presence timed out; retry".to_owned())?
            .unwrap_or_else(|_| Err("connection lost; retry".to_owned()))
    }
}

fn dead_client_err() -> ErrBody {
    ErrBody {
        code: ErrorCode::ServerInternal,
        message: "client stopped (fatal join failure — check token/grant)".into(),
        retryable: false,
        retry_after_ms: None,
    }
}

// ---- 액터 내부 ----

struct OutboxEntry {
    client_key: ClientKey,
    envelope: Envelope,
    resp: Option<oneshot::Sender<Result<MessageId, ErrBody>>>,
}

enum Pending {
    Pub(ClientKey),
    Fetch(oneshot::Sender<Result<Vec<Envelope>, String>>),
    Presence(oneshot::Sender<Result<Vec<PresenceEntry>, String>>),
}

struct InboxItem {
    server_seq: u64,
    envelope: Envelope,
}

struct Actor {
    opts: ClientOptions,
    seq: u64,
    outbox: Vec<OutboxEntry>,
    pending: HashMap<u64, Pending>,
    inbox: VecDeque<InboxItem>,
    queued_ids: HashSet<String>,
    /// 소비 완료 id — 재전달 중복을 즉시 ACK로 흡수 (13.3의 수신 방향).
    consumed: VecDeque<String>,
    consumed_set: HashSet<String>,
    waiters: Vec<(RecvFilter, oneshot::Sender<Envelope>)>,
    last_pong: Instant,
}

const CONSUMED_CAP: usize = 2048;

impl Actor {
    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    fn build_envelope(&self, spec: &PublishSpec, key: &ClientKey) -> Result<Envelope, String> {
        Ok(Envelope {
            v: brevduva_protocol::PROTOCOL_VERSION,
            id: None,
            ts: None,
            client_key: key.clone(),
            from: Ident::parse(&self.opts.agent).map_err(|e| e.to_string())?,
            to: Address::parse(&spec.to).map_err(|e| e.to_string())?,
            kind: spec.kind,
            correlation_id: match &spec.correlation_id {
                Some(c) => Some(MessageId::parse(c).map_err(|e| e.to_string())?),
                None => None,
            },
            expects: spec.expects,
            ttl_ms: spec.ttl_ms,
            hops: spec.hops,
            content_type: spec.content_type.clone(),
            payload: spec.payload.clone(),
            payload_ref: None,
            meta: spec.meta.clone(),
        })
    }

    fn mark_consumed(&mut self, id: String) {
        if self.consumed_set.insert(id.clone()) {
            self.consumed.push_back(id);
            if self.consumed.len() > CONSUMED_CAP
                && let Some(old) = self.consumed.pop_front()
            {
                self.consumed_set.remove(&old);
            }
        }
    }

    /// 대기자와 큐를 맞춰본다 — 전달 성공 시에만 ACK (유실 방지: 전달 → ACK 순서).
    async fn satisfy_waiters(&mut self, ws: &mut Ws) {
        self.waiters.retain(|(_, tx)| !tx.is_closed());
        let mut w = 0;
        while w < self.waiters.len() {
            let matched = self
                .inbox
                .iter()
                .position(|item| self.waiters[w].0.matches(&item.envelope));
            let Some(pos) = matched else {
                w += 1;
                continue;
            };
            let item = self.inbox.remove(pos).expect("position valid");
            let (_, tx) = self.waiters.remove(w);
            let id = item.envelope.id.as_ref().map(|i| i.as_str().to_owned());
            if tx.send(item.envelope.clone()).is_ok() {
                let _ = send_frame(
                    ws,
                    &ClientFrame {
                        seq: None,
                        re: Some(item.server_seq),
                        op: ClientOp::Ack,
                    },
                )
                .await;
                if let Some(id) = id {
                    self.queued_ids.remove(&id);
                    self.mark_consumed(id);
                }
                // 같은 인덱스에서 계속 (다음 대기자)
            } else {
                // 수신자가 타임아웃으로 사라짐 — 메시지를 큐에 되돌린다 (유실 금지)
                self.inbox.insert(pos.min(self.inbox.len()), item);
            }
        }
    }

    async fn handle_deliver(&mut self, ws: &mut Ws, server_seq: u64, env: Envelope) {
        // 프레즌스 잡담은 어댑터 계층에서 소비하지 않는다 — PRESENCE 조회가 진실 (5.3)
        if env.kind == Kind::Event
            && let Some(payload) = env.payload.as_deref()
            && serde_json::from_str::<Value>(payload)
                .ok()
                .is_some_and(|v| v["type"] == "presence")
        {
            let _ = send_frame(
                ws,
                &ClientFrame {
                    seq: None,
                    re: Some(server_seq),
                    op: ClientOp::Ack,
                },
            )
            .await;
            return;
        }
        let Some(id) = env.id.as_ref().map(|i| i.as_str().to_owned()) else {
            return; // id 없는 전달은 프로토콜 위반 — 무시
        };
        if self.consumed_set.contains(&id) {
            // 이미 소비한 메시지의 재전달 — 즉시 ACK로 사슬을 끊는다
            let _ = send_frame(
                ws,
                &ClientFrame {
                    seq: None,
                    re: Some(server_seq),
                    op: ClientOp::Ack,
                },
            )
            .await;
            return;
        }
        if self.queued_ids.contains(&id) {
            // 미소비 재전달 — ACK 대상 seq만 갱신
            if let Some(item) = self
                .inbox
                .iter_mut()
                .find(|i| i.envelope.id.as_ref().is_some_and(|x| x.as_str() == id))
            {
                item.server_seq = server_seq;
            }
            return;
        }
        self.queued_ids.insert(id);
        self.inbox.push_back(InboxItem {
            server_seq,
            envelope: env,
        });
        self.satisfy_waiters(ws).await;
    }

    async fn handle_cmd(&mut self, ws: &mut Ws, cmd: Cmd) {
        match cmd {
            Cmd::Publish(spec, resp) => {
                let key = ClientKey::generate();
                match self.build_envelope(&spec, &key) {
                    Ok(envelope) => {
                        let seq = self.next_seq();
                        self.pending.insert(seq, Pending::Pub(key.clone()));
                        let frame = ClientFrame {
                            seq: Some(seq),
                            re: None,
                            op: ClientOp::Pub(envelope.clone()),
                        };
                        self.outbox.push(OutboxEntry {
                            client_key: key,
                            envelope,
                            resp: Some(resp),
                        });
                        let _ = send_frame(ws, &frame).await; // 실패 시 재연결 경로가 재발행
                    }
                    Err(reason) => {
                        let _ = resp.send(Err(ErrBody {
                            code: ErrorCode::FrameInvalid,
                            message: reason,
                            retryable: false,
                            retry_after_ms: None,
                        }));
                    }
                }
            }
            Cmd::Recv(filter, tx) => {
                self.waiters.push((filter, tx));
                self.satisfy_waiters(ws).await;
            }
            Cmd::Fetch {
                after_id,
                limit,
                resp,
            } => {
                let seq = self.next_seq();
                self.pending.insert(seq, Pending::Fetch(resp));
                let frame = ClientFrame {
                    seq: Some(seq),
                    re: None,
                    op: ClientOp::Fetch {
                        topics: None,
                        after_id: after_id.and_then(|s| MessageId::parse(&s).ok()),
                        after_ts: None,
                        limit,
                    },
                };
                let _ = send_frame(ws, &frame).await;
            }
            Cmd::Presence(resp) => {
                let seq = self.next_seq();
                self.pending.insert(seq, Pending::Presence(resp));
                let frame = ClientFrame {
                    seq: Some(seq),
                    re: None,
                    op: ClientOp::Presence,
                };
                let _ = send_frame(ws, &frame).await;
            }
        }
    }

    fn resolve_ok(&mut self, re: u64, body: OkBody) {
        match self.pending.remove(&re) {
            Some(Pending::Pub(key)) => {
                if let Some(pos) = self.outbox.iter().position(|e| e.client_key == key) {
                    let mut entry = self.outbox.remove(pos);
                    if let (Some(resp), Some(id)) = (entry.resp.take(), body.id) {
                        let _ = resp.send(Ok(id));
                    }
                }
            }
            Some(Pending::Fetch(resp)) => {
                let _ = resp.send(Ok(body.messages.unwrap_or_default()));
            }
            Some(Pending::Presence(resp)) => {
                let _ = resp.send(Ok(body.presence.unwrap_or_default()));
            }
            None => {}
        }
    }

    fn resolve_err(&mut self, re: u64, body: ErrBody) {
        match self.pending.remove(&re) {
            Some(Pending::Pub(key)) => {
                // 서버가 명시적으로 거부 — 정직하게 호출자에게 (13.4). 재시도 판단은 에이전트 몫
                if let Some(pos) = self.outbox.iter().position(|e| e.client_key == key) {
                    let mut entry = self.outbox.remove(pos);
                    if let Some(resp) = entry.resp.take() {
                        let _ = resp.send(Err(body));
                    }
                }
            }
            Some(Pending::Fetch(resp)) => {
                let _ = resp.send(Err(body.message));
            }
            Some(Pending::Presence(resp)) => {
                let _ = resp.send(Err(body.message));
            }
            None => {}
        }
    }

    /// 연결 유실 시: 응답 없는 PUB는 outbox에 남아 재발행되고, 조회성 요청은 오류로 해소.
    fn on_disconnect(&mut self) {
        for (_, pending) in self.pending.drain() {
            match pending {
                Pending::Pub(_) => {} // outbox가 진실 — 재발행된다
                Pending::Fetch(resp) => {
                    let _ = resp.send(Err("connection lost; retry".to_owned()));
                }
                Pending::Presence(resp) => {
                    let _ = resp.send(Err("connection lost; retry".to_owned()));
                }
            }
        }
    }
}

async fn send_frame(ws: &mut Ws, frame: &ClientFrame) -> anyhow::Result<()> {
    let text = serde_json::to_string(frame).expect("client frame serializes");
    ws.send(WsMessage::Text(text.into()))
        .await
        .context("ws send")
}

fn ws_url(server: &str) -> String {
    let base = server.trim_end_matches('/');
    let ws = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        format!("ws://{base}")
    };
    format!("{ws}/v1/ws")
}

/// full jitter — random(0, min(60s, 1s × 2^attempt)) (13.2). 의존성 없는 시간 기반 난수.
fn backoff(attempt: u32) -> Duration {
    let cap_ms = 60_000u128.min(1000u128.saturating_mul(1u128 << attempt.min(16)));
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u128)
        .unwrap_or(1);
    Duration::from_millis((nanos % cap_ms.max(1)) as u64)
}

async fn actor(opts: ClientOptions, mut cmds: mpsc::Receiver<Cmd>) {
    let mut state = Actor {
        opts,
        seq: 0,
        outbox: Vec::new(),
        pending: HashMap::new(),
        inbox: VecDeque::new(),
        queued_ids: HashSet::new(),
        consumed: VecDeque::new(),
        consumed_set: HashSet::new(),
        waiters: Vec::new(),
        last_pong: Instant::now(),
    };
    let mut attempt: u32 = 0;
    loop {
        match connect_and_join(&mut state).await {
            Ok((ws, backlog)) => {
                attempt = 0;
                match run_connection(&mut state, ws, backlog, &mut cmds).await {
                    ConnEnd::Reconnect => state.on_disconnect(),
                    ConnEnd::TakenOver => {
                        state.on_disconnect();
                        if state.opts.takeover_standby {
                            // 새 세션과 자리 다툼 금지 (2.2) — 자리가 빌 때까지 대기
                            tracing::info!("session taken over — entering standby");
                            standby_until_free(&state.opts).await;
                            tracing::info!("agent slot free — resuming");
                        }
                        // 대화형(기본): 즉시 재접속 = 최신 연결이 자리를 가진다
                    }
                    ConnEnd::Closed => return,
                }
            }
            Err(e) => {
                let msg = e.to_string();
                if let Some(reason) = msg.strip_prefix("FATAL: ") {
                    // JOIN이 비재시도 오류로 거부됨 (무효 토큰·grant 없음 등) — 재시도 무의미
                    tracing::error!(%reason, "fatal join failure — client stopping");
                    for entry in state.outbox.drain(..) {
                        if let Some(resp) = entry.resp {
                            let _ = resp.send(Err(ErrBody {
                                code: ErrorCode::AuthInvalidToken,
                                message: reason.to_owned(),
                                retryable: false,
                                retry_after_ms: None,
                            }));
                        }
                    }
                    return;
                }
                tracing::warn!(error = %e, attempt, "connect failed");
            }
        }
        attempt += 1;
        tokio::time::sleep(backoff(attempt)).await;
    }
}

// 치명(JOIN 비재시도 거부)은 connect_and_join의 "FATAL: " 오류 경로가 담당한다
enum ConnEnd {
    Reconnect,
    /// 다른 세션이 자리를 가져감 (`agent/session-conflict` 수신, 2.2).
    TakenOver,
    Closed,
}

/// standby: long-poll PRESENCE 프로브(세션·큐 무접촉)로 자리가 빌 때까지 대기.
/// 오류(서버 불가 등)는 "자리 비어 있음"으로 간주 — 재접속 경로의 백오프가 뒷일을 맡는다.
async fn standby_until_free(opts: &ClientOptions) {
    let http = reqwest::Client::new();
    loop {
        tokio::time::sleep(opts.standby_probe).await;
        let occupied = async {
            let resp: ServerFrame = http
                .post(format!(
                    "{}/v1/frames?channel={}",
                    opts.server.trim_end_matches('/'),
                    opts.channel
                ))
                .bearer_auth(&opts.token)
                .json(&ClientFrame {
                    seq: Some(1),
                    re: None,
                    op: ClientOp::Presence,
                })
                .send()
                .await
                .ok()?
                .json()
                .await
                .ok()?;
            let ServerOp::Ok(body) = resp.op else {
                return None;
            };
            let mine = body
                .presence?
                .into_iter()
                .find(|e| e.agent.as_str() == opts.agent)?;
            Some(mine.state == brevduva_protocol::PresenceState::Online)
        }
        .await;
        if !occupied.unwrap_or(false) {
            return;
        }
    }
}

/// 접속 + JOIN(멱등, 13.2). JOIN OK 이전에 도착한 DELIVER는 backlog로 넘긴다.
async fn connect_and_join(state: &mut Actor) -> anyhow::Result<(Ws, Vec<ServerFrame>)> {
    let (mut ws, _) = connect_async(ws_url(&state.opts.server))
        .await
        .context("ws connect")?;
    let join_seq = state.next_seq();
    let caps = Capabilities {
        agent: Ident::parse(&state.opts.agent).context("agent name")?,
        description: state.opts.description.clone(),
        max_inline_bytes: 262_144,
        content_types: vec!["text/*".into(), "application/json".into()],
        encodings: vec!["json".into()],
        modes: vec![brevduva_protocol::ReceiveMode::Push],
        meta: Map::new(),
    };
    send_frame(
        &mut ws,
        &ClientFrame {
            seq: Some(join_seq),
            re: None,
            op: ClientOp::Join {
                channel: Ident::parse(&state.opts.channel).context("channel name")?,
                token: state.opts.token.clone(),
                capabilities: caps,
            },
        },
    )
    .await?;
    let mut backlog = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let frame = timeout(deadline - Instant::now(), ws.next())
            .await
            .context("join timeout")?
            .context("connection closed during join")?
            .context("ws read")?;
        let WsMessage::Text(text) = frame else {
            continue;
        };
        let frame: ServerFrame = serde_json::from_str(text.as_str()).context("bad server frame")?;
        match &frame.op {
            ServerOp::Ok(_) if frame.re == Some(join_seq) => {
                state.last_pong = Instant::now();
                // 미확인 PUB 재발행 (13.3 — 같은 client_key)
                let mut frames = Vec::new();
                for entry in &state.outbox {
                    let seq = {
                        state.seq += 1;
                        state.seq
                    };
                    state
                        .pending
                        .insert(seq, Pending::Pub(entry.client_key.clone()));
                    frames.push(ClientFrame {
                        seq: Some(seq),
                        re: None,
                        op: ClientOp::Pub(entry.envelope.clone()),
                    });
                }
                for f in &frames {
                    send_frame(&mut ws, f).await?;
                }
                return Ok((ws, backlog));
            }
            ServerOp::Err(body) if frame.re == Some(join_seq) => {
                if body.retryable {
                    anyhow::bail!("join rejected (retryable): {}", body.message);
                }
                // 치명 — 상위에서 Fatal 처리하도록 특수 문자열로 구분하지 않고 오류 타입화
                return Err(anyhow::anyhow!("FATAL: {}", body.message));
            }
            _ => backlog.push(frame),
        }
    }
}

async fn run_connection(
    state: &mut Actor,
    mut ws: Ws,
    backlog: Vec<ServerFrame>,
    cmds: &mut mpsc::Receiver<Cmd>,
) -> ConnEnd {
    for frame in backlog {
        if let ServerOp::Deliver(env) = frame.op
            && let Some(seq) = frame.seq
        {
            state.handle_deliver(&mut ws, seq, *env).await;
        }
    }
    let mut ping_tick = interval(Duration::from_secs(20));
    ping_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            cmd = cmds.recv() => match cmd {
                Some(cmd) => state.handle_cmd(&mut ws, cmd).await,
                None => return ConnEnd::Closed, // 핸들 전부 드롭 — 종료
            },
            _ = ping_tick.tick() => {
                // 13.1: 2회 연속 무응답(≈45s) 판정
                if state.last_pong.elapsed() > Duration::from_secs(45) {
                    tracing::warn!("heartbeat lost; reconnecting");
                    return ConnEnd::Reconnect;
                }
                let seq = state.next_seq();
                if send_frame(&mut ws, &ClientFrame { seq: Some(seq), re: None, op: ClientOp::Ping }).await.is_err() {
                    return ConnEnd::Reconnect;
                }
            },
            incoming = ws.next() => {
                let Some(Ok(msg)) = incoming else { return ConnEnd::Reconnect };
                let WsMessage::Text(text) = msg else { continue };
                let Ok(frame) = serde_json::from_str::<ServerFrame>(text.as_str()) else {
                    tracing::warn!("unparsable server frame; ignoring");
                    continue;
                };
                state.last_pong = Instant::now(); // 어떤 프레임이든 살아있음의 증거
                match frame.op {
                    ServerOp::Ok(body) => if let Some(re) = frame.re { state.resolve_ok(re, body); },
                    ServerOp::Err(body) => {
                        if let Some(re) = frame.re {
                            state.resolve_err(re, body);
                        } else if body.code == ErrorCode::AgentSessionConflict {
                            // 테이크오버 통지 (2.2) — 네트워크 장애와 구별되는 명시적 신호
                            return ConnEnd::TakenOver;
                        } else if body.code == ErrorCode::FrameInvalid {
                            tracing::warn!(message = %body.message, "server rejected a frame");
                        }
                    }
                    ServerOp::Deliver(env) => {
                        if let Some(seq) = frame.seq {
                            state.handle_deliver(&mut ws, seq, *env).await;
                        }
                    }
                    ServerOp::Ping => {
                        let _ = send_frame(&mut ws, &ClientFrame { seq: None, re: frame.seq, op: ClientOp::Pong }).await;
                    }
                    ServerOp::Pong => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_url_derivation() {
        assert_eq!(ws_url("http://1.2.3.4:8080"), "ws://1.2.3.4:8080/v1/ws");
        assert_eq!(
            ws_url("https://brv.example.com/"),
            "wss://brv.example.com/v1/ws"
        );
    }

    #[test]
    fn backoff_respects_cap() {
        for attempt in 0..20 {
            assert!(
                backoff(attempt) <= Duration::from_secs(60),
                "attempt {attempt}"
            );
        }
    }

    #[test]
    fn recv_filter_matching() {
        let mut env = Envelope {
            v: 1,
            id: None,
            ts: None,
            client_key: ClientKey::generate(),
            from: Ident::parse("a").unwrap(),
            to: Address::parse("agent:b").unwrap(),
            kind: Kind::Reply,
            correlation_id: Some(MessageId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap()),
            expects: None,
            ttl_ms: None,
            hops: 0,
            content_type: "text/plain".into(),
            payload: None,
            payload_ref: None,
            meta: Map::new(),
        };
        assert!(RecvFilter::Any.matches(&env));
        assert!(RecvFilter::Correlation("01ARZ3NDEKTSV4RRFFQ69G5FAV".into()).matches(&env));
        assert!(!RecvFilter::Correlation("01BX5ZZKBKACTAV9WEVGEMMVRZ".into()).matches(&env));
        env.correlation_id = None;
        assert!(!RecvFilter::Correlation("01ARZ3NDEKTSV4RRFFQ69G5FAV".into()).matches(&env));
    }
}
