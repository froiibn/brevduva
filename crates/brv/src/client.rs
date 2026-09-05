// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

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

/// 유휴 파킹 기본값 — ack_wait(기본 30s)×포이즌 임계(5)로 격리되기 한참 전에 자리를 내려놓는다.
pub const DEFAULT_IDLE_PARK: Duration = Duration::from_secs(60);

/// 데몬 모드의 토큰 재읽기 (2026-09-02, 맥북 실사고): JOIN이 토큰 거부로 실패하면 죽지 않고
/// 정지한 채 이 콜백으로 저장소의 토큰을 다시 읽어 재시도한다 — 같은 머신에서 재enroll
/// (토큰 회전)하면 재기동 없이 자가 복구. None(대화형 어댑터)이면 종전대로 즉시 종료.
#[derive(Clone)]
pub struct TokenReload(pub std::sync::Arc<dyn Fn() -> Option<String> + Send + Sync>);

impl std::fmt::Debug for TokenReload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenReload(..)")
    }
}

/// 클라이언트 접속 상태 — 데몬이 상태 파일(`brv status`)로 노출한다 (2026-09-02, 맥북 실사고:
/// "죽은 채 살아 있음"을 밖에서 볼 수단이 로그 tail뿐이었다).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ClientState {
    Connecting,
    Connected,
    Reconnecting {
        attempt: u32,
    },
    /// 다른 세션이 자리를 가져가 대기 중 (2.2).
    Standby,
    /// 유휴 파킹 — 메시지는 서버 큐에 (2026-09-01).
    Parked,
    /// JOIN이 비재시도 오류로 거부돼 정지 — 토큰 재읽기·백오프 재시도 중 (데몬 모드).
    Suspended {
        reason: String,
        retry_in_s: u64,
    },
    /// 데몬이 접속을 **보류** 중 — 깨우기 사전 점검 실패 (2026-09-03: "깨울 수 없으면 자리를
    /// 잡지 않는다"). 클라이언트가 아니라 데몬이 기록하는 상태: 메시지는 서버 큐에 남고
    /// 프레즌스는 idle로 정직하다. 재점검 통과 시 접속.
    WakeUnavailable {
        reason: String,
        retry_in_s: u64,
    },
    /// 운영자 일시정지 (`brv daemon pause`, 2026-09-03) — 데몬이 자리를 내려놓아 메시지는
    /// 서버 큐에 남는다. 대화형 세션이 채널을 직접 맡을 때 쓰는 정직한 수단 (구 `never` 대체).
    Paused {
        until_unix: u64,
    },
    /// 종료 — 대화형 모드의 치명 오류, 또는 핸들 전부 드롭.
    Stopped {
        reason: String,
    },
}

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
    /// **유휴 파킹** (2026-09-01, 방치 실측의 근본 수정): 명령·대기자·미해결 작업이 전부 없는
    /// 상태가 이 시간을 넘으면 접속을 스스로 내려놓는다 — 메시지가 "듣지 않는 세션의 버퍼"에
    /// 배달돼 미소비 재전달(ack_wait)로 격리 예산을 태우는 대신, 서버 큐에 안전하게 남는다.
    /// 다음 명령이 오면 자동 재접속(재JOIN)해 밀린 것부터 받는다. 단, 확인 유보분(unacked —
    /// 데몬 wake 실패 등 처리 실패 신호)이 있으면 파킹하지 않는다: 그 재전달·포이즌 가시화는
    /// 페이즈 20의 의도된 실패 표면이다. None = 파킹 없음 (listen 등 상시 대기자 경로).
    pub idle_park: Option<Duration>,
    /// 토큰 거부(치명 JOIN 실패) 시 종료 대신 정지·재시도 — 데몬 모드 (위 `TokenReload`).
    pub token_reload: Option<TokenReload>,
    /// 정지 재시도 기본 간격 — 기본 30s, 단계적으로 최대 30배(15분)까지 (`fatal_backoff`).
    pub fatal_retry_base: Duration,
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
            idle_park: None,
            token_reload: None,
            fatal_retry_base: Duration::from_secs(30),
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
    /// claim-check 참조 (3.2) — 보통 publish()의 투명 전환이 채운다 (직접 지정도 가능).
    pub payload_ref: Option<brevduva_protocol::PayloadRef>,
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
            payload_ref: None,
        }
    }
}

/// `wait_for_reply`의 결과 — 최종 답이 왔거나, 진행 알림만 온 채(또는 아무것도 없이) 시간이 다했거나.
#[derive(Debug)]
pub enum ReplyWait {
    Replied {
        /// Box: 봉투가 크고(수백 바이트) Pending 변형은 작다 — variant 크기 차이 경고 회피.
        reply: Box<Envelope>,
        /// 답 전에 온 마지막 진행 알림 (`report{status:"in-progress"}`) — 있으면 함께 넘긴다.
        progress: Option<Envelope>,
    },
    Pending {
        progress: Option<Envelope>,
    },
}

/// 히스토리 조회 조건 (FETCH). 기본은 과거→현재(`after_id` 커서); `newest_first`면 최신부터
/// 역순이고 커서는 `before_id`다 (2026-09-05).
#[derive(Debug, Clone, Default)]
pub struct FetchQuery {
    pub after_id: Option<String>,
    pub before_id: Option<String>,
    pub newest_first: bool,
    pub limit: Option<u32>,
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
    // bool = auto_ack: false면 전달만 하고 확인(ACK)을 유보한다 — Confirm이 마무리 (페이즈 20)
    Recv(RecvFilter, bool, oneshot::Sender<(Envelope, u64)>),
    // recv_manual로 받은 전달의 확인 — 깨우기 성공 등 "처리 보장" 시점에 호출
    Confirm(u64),
    Fetch {
        query: FetchQuery,
        resp: oneshot::Sender<Result<Vec<Envelope>, String>>,
    },
    Presence(oneshot::Sender<Result<Vec<PresenceEntry>, String>>),
}

/// 인라인 임계값의 클라이언트 측 선검사 기준 (12.2 기본값) — 진실은 서버.
/// 서버가 더 엄격하면 msg/too-large가 오고, 그때 업로드-재시도로 적응한다.
const INLINE_LIMIT: usize = 262_144;

/// 재연결·큐·ACK를 감춘 클라이언트 핸들. clone 가능.
#[derive(Clone)]
pub struct Client {
    cmds: mpsc::Sender<Cmd>,
    /// 투명 claim-check(페이즈 17)의 HTTP 업로드용 접속 정보 — 액터 밖에서 업로드해야
    /// 대용량 전송이 WS 루프(PING·전달)를 막지 않는다.
    server: String,
    channel: String,
    token: String,
    state_rx: tokio::sync::watch::Receiver<ClientState>,
}

impl Client {
    pub fn connect(opts: ClientOptions) -> Self {
        let (tx, rx) = mpsc::channel(64);
        let (state_tx, state_rx) = tokio::sync::watch::channel(ClientState::Connecting);
        let (server, channel, token) = (
            opts.server.clone(),
            opts.channel.clone(),
            opts.token.clone(),
        );
        tokio::spawn(actor(opts, rx, state_tx));
        Self {
            cmds: tx,
            server,
            channel,
            token,
            state_rx,
        }
    }

    /// 접속 상태 관찰 — 데몬 상태 파일용 (변경 알림은 `changed()`).
    pub fn state(&self) -> tokio::sync::watch::Receiver<ClientState> {
        self.state_rx.clone()
    }

    /// 액터 생존 여부. 죽은 클라이언트의 recv는 시간 초과와 같은 None을 돌려주므로 호출자
    /// (데몬 루프)가 이걸로 구분한다 — 2026-09-02 맥북 실사고: 구분 못 해 죽은 채 무한 재대기.
    pub fn is_alive(&self) -> bool {
        !self.cmds.is_closed()
    }

    /// 발행 — 단절 중이면 재연결 후 같은 client_key로 재시도된다 (13.3).
    /// 호출자는 어댑터 정직성 규약(13.4)에 따라 자체 타임아웃을 걸어라.
    ///
    /// **투명 claim-check (페이즈 17, 3.2)**: 임계값 초과 페이로드는 blob으로 올리고 참조만
    /// 싣는다 — 호출자는 크기 제한을 체감하지 않는다. 선검사(기본 256KB)를 통과했는데도
    /// 서버가 msg/too-large를 주면(서버 설정이 더 엄격) 한 번 업로드 후 재시도한다.
    pub async fn publish(&self, spec: PublishSpec) -> Result<MessageId, ErrBody> {
        // spawn: 호출자가 타임아웃으로 future를 떨어뜨려도(어댑터 정직성 규약 13.4의 10s 등)
        // 업로드·발행은 끝까지 진행된다 — "미확인이지만 곧 발행됨" 안내가 거짓이 되지 않게.
        let this = self.clone();
        let handle = tokio::spawn(async move { this.publish_flow(spec).await });
        handle.await.unwrap_or_else(|_| Err(dead_client_err()))
    }

    async fn publish_flow(&self, mut spec: PublishSpec) -> Result<MessageId, ErrBody> {
        if spec.payload_ref.is_none()
            && spec
                .payload
                .as_ref()
                .is_some_and(|p| p.len() > INLINE_LIMIT)
        {
            self.offload(&mut spec).await?;
        }
        let first = self.publish_raw(spec.clone()).await;
        match &first {
            Err(e) if e.code == ErrorCode::MsgTooLarge && spec.payload.is_some() => {
                self.offload(&mut spec).await?;
                self.publish_raw(spec).await
            }
            _ => first,
        }
    }

    /// 페이로드 → blob 업로드 + 참조 전환 (payload는 비운다).
    async fn offload(&self, spec: &mut PublishSpec) -> Result<(), ErrBody> {
        let payload = spec.payload.take().unwrap_or_default();
        let r = upload_blob(
            &self.server,
            &self.channel,
            &self.token,
            &spec.content_type,
            payload.into_bytes(),
        )
        .await
        .map_err(|e| ErrBody {
            code: ErrorCode::ServerInternal,
            message: format!("claim-check upload failed: {e}"),
            retryable: true,
            retry_after_ms: None,
        })?;
        spec.payload_ref = Some(r);
        Ok(())
    }

    async fn publish_raw(&self, spec: PublishSpec) -> Result<MessageId, ErrBody> {
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
    /// correlation의 **최종** 반응을 기다린다 (9장). 진행 알림(3.1 — `report{status:"in-progress"}`,
    /// 그리고 JSON이 아니거나 status 없는 report)은 답이 아니다 — 소비하고(큐에 남겨 두면 미소비
    /// 재전달 예산을 태운다) 진행 정보로 넘긴 뒤 남은 시간만큼 계속 기다린다. 발단(2026-09-05 실측): 0.6.22가 넣은 착수 알림을 `request`가 답으로
    /// 돌려줬다 — 실제 답은 그 뒤에 오는데 세션은 이미 넘어간 뒤였다.
    pub async fn recv_reply(&self, correlation: &str, wait: Duration) -> ReplyWait {
        let deadline = Instant::now() + wait;
        let mut progress = None;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return ReplyWait::Pending { progress };
            }
            match self
                .recv(RecvFilter::Correlation(correlation.to_owned()), left)
                .await
            {
                Some(env) if env.is_progress_report() => progress = Some(env),
                Some(reply) => {
                    return ReplyWait::Replied {
                        reply: Box::new(reply),
                        progress,
                    };
                }
                None => return ReplyWait::Pending { progress },
            }
        }
    }

    pub async fn recv(&self, filter: RecvFilter, wait: Duration) -> Option<Envelope> {
        let (tx, rx) = oneshot::channel();
        self.cmds.send(Cmd::Recv(filter, true, tx)).await.ok()?;
        timeout(wait, rx).await.ok()?.ok().map(|(env, _)| env)
    }

    /// 확인 유보 수신 (페이즈 20) — 전달만 받고 ACK를 미룬다. 반환된 토큰으로 `confirm`을
    /// 호출해야 소비가 확정되며, 확인 없이는 ack_wait 후 재전달된다 (처리 실패 = 자동 재시도).
    /// 데몬의 "깨우기 성공 후 확인" 용도 — 2026-08-29 wake 실사고(도장 후 깨우기 실패로
    /// 메시지가 큐에서 이탈)의 근본 수정.
    pub async fn recv_manual(&self, filter: RecvFilter, wait: Duration) -> Option<(Envelope, u64)> {
        let (tx, rx) = oneshot::channel();
        self.cmds.send(Cmd::Recv(filter, false, tx)).await.ok()?;
        timeout(wait, rx).await.ok()?.ok()
    }

    /// recv_manual 전달의 소비 확정 — best-effort (재접속으로 무효면 재전달이 대신한다).
    pub async fn confirm(&self, token: u64) {
        let _ = self.cmds.send(Cmd::Confirm(token)).await;
    }

    /// 과거→현재 조회 (`after_id` 커서) — `fetch_query`의 단축형.
    pub async fn fetch(
        &self,
        after_id: Option<String>,
        limit: Option<u32>,
        wait: Duration,
    ) -> Result<Vec<Envelope>, String> {
        self.fetch_query(
            FetchQuery {
                after_id,
                limit,
                ..FetchQuery::default()
            },
            wait,
        )
        .await
    }

    /// 히스토리 조회 — 방향·커서를 `FetchQuery`로 (2026-09-05, 역순 조회 추가).
    pub async fn fetch_query(
        &self,
        query: FetchQuery,
        wait: Duration,
    ) -> Result<Vec<Envelope>, String> {
        let (tx, rx) = oneshot::channel();
        self.cmds
            .send(Cmd::Fetch { query, resp: tx })
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

/// 수신 대기자: (필터, 자동 ACK 여부, 전달 통로 — 봉투와 confirm 토큰).
type RecvWaiter = (RecvFilter, bool, oneshot::Sender<(Envelope, u64)>);

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
    waiters: Vec<RecvWaiter>,
    /// 확인 유보 전달 (페이즈 20): server_seq → 메시지 id. 연결마다 seq 공간이 새로 시작하므로
    /// 재접속 시 비운다 — 미확인분은 ack_wait 재전달이 다시 가져온다 (유실 없음).
    unacked: HashMap<u64, String>,
    last_pong: Instant,
    /// 접속 상태 발행 (2026-09-02) — `Client::state()`가 구독.
    state_tx: tokio::sync::watch::Sender<ClientState>,
}

impl Actor {
    fn set_state(&self, s: ClientState) {
        self.state_tx.send_replace(s);
    }
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
            payload_ref: spec.payload_ref.clone(),
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
        self.waiters.retain(|(_, _, tx)| !tx.is_closed());
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
            let (_, auto_ack, tx) = self.waiters.remove(w);
            let id = item.envelope.id.as_ref().map(|i| i.as_str().to_owned());
            if tx.send((item.envelope.clone(), item.server_seq)).is_ok() {
                if auto_ack {
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
                } else {
                    // 확인 유보 (페이즈 20) — Confirm이 올 때까지 서버 큐의 소유가 유지된다.
                    // 확인이 안 오면 ack_wait 후 재전달 (처리 실패 = 자동 재시도의 재료)
                    if let Some(id) = id {
                        self.queued_ids.remove(&id);
                        self.unacked.insert(item.server_seq, id);
                    }
                }
                // 같은 인덱스에서 계속 (다음 대기자)
            } else {
                // 수신자가 타임아웃으로 사라짐 — 메시지를 큐에 되돌린다 (유실 금지)
                self.inbox.insert(pos.min(self.inbox.len()), item);
            }
        }
    }

    /// recv_manual 전달의 확인 (페이즈 20) — ACK + 소비 표시 + 그 사이 도착한 중복 재전달 정리.
    async fn handle_confirm(&mut self, ws: &mut Ws, seq: u64) {
        let Some(id) = self.unacked.remove(&seq) else {
            return; // 재접속으로 이미 무효 — 재전달이 새로 온다 (at-least-once)
        };
        let _ = send_frame(
            ws,
            &ClientFrame {
                seq: None,
                re: Some(seq),
                op: ClientOp::Ack,
            },
        )
        .await;
        self.mark_consumed(id.clone());
        // 확인 전에 ack_wait 재전달로 들어온 같은 id의 중복분 흡수 — ACK 후 제거
        let mut dup_seqs = Vec::new();
        self.inbox.retain(|item| {
            let dup = item.envelope.id.as_ref().is_some_and(|x| x.as_str() == id);
            if dup {
                dup_seqs.push(item.server_seq);
            }
            !dup
        });
        for dup in dup_seqs {
            self.queued_ids.remove(&id);
            let _ = send_frame(
                ws,
                &ClientFrame {
                    seq: None,
                    re: Some(dup),
                    op: ClientOp::Ack,
                },
            )
            .await;
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
            Cmd::Recv(filter, auto_ack, tx) => {
                self.waiters.push((filter, auto_ack, tx));
                self.satisfy_waiters(ws).await;
            }
            Cmd::Confirm(seq) => {
                self.handle_confirm(ws, seq).await;
            }
            Cmd::Fetch { query, resp } => {
                let seq = self.next_seq();
                self.pending.insert(seq, Pending::Fetch(resp));
                let frame = ClientFrame {
                    seq: Some(seq),
                    re: None,
                    op: ClientOp::Fetch {
                        topics: None,
                        after_id: query.after_id.and_then(|s| MessageId::parse(&s).ok()),
                        after_ts: None,
                        before_id: query.before_id.and_then(|s| MessageId::parse(&s).ok()),
                        newest_first: query.newest_first,
                        limit: query.limit,
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
        // 확인 유보분은 이 연결의 seq에 묶여 있었다 — 재전달이 새 seq로 다시 온다
        self.unacked.clear();
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

async fn actor(
    opts: ClientOptions,
    mut cmds: mpsc::Receiver<Cmd>,
    state_tx: tokio::sync::watch::Sender<ClientState>,
) {
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
        unacked: HashMap::new(),
        last_pong: Instant::now(),
        state_tx,
    };
    let mut attempt: u32 = 0;
    let mut fatal_attempt: u32 = 0;
    let mut resume_cmd: Option<Cmd> = None;
    let dropped = ClientState::Stopped {
        reason: "all handles dropped".to_owned(),
    };
    loop {
        match connect_and_join(&mut state).await {
            Ok((ws, backlog)) => {
                attempt = 0;
                fatal_attempt = 0;
                state.set_state(ClientState::Connected);
                match run_connection(&mut state, ws, backlog, resume_cmd.take(), &mut cmds).await {
                    ConnEnd::Reconnect => state.on_disconnect(),
                    ConnEnd::TakenOver => {
                        state.on_disconnect();
                        if state.opts.takeover_standby {
                            // 새 세션과 자리 다툼 금지 (2.2) — 자리가 빌 때까지 대기
                            tracing::info!("session taken over — entering standby");
                            state.set_state(ClientState::Standby);
                            if !standby_until_free(&state.opts, &cmds).await {
                                state.set_state(dropped); // 핸들 전부 드롭 — 재JOIN 없이 종료
                                return;
                            }
                            tracing::info!("agent slot free — resuming");
                        }
                        // 대화형(기본): 즉시 재접속 = 최신 연결이 자리를 가진다
                    }
                    ConnEnd::Park => {
                        // 유휴 파킹 (2026-09-01): 미소비 버퍼는 서버 큐에 반납한다 —
                        // 로컬 사본을 남기면 재JOIN 후 낡은 seq로 ACK하는 어긋남이 생긴다.
                        // ACK 안 한 전달분은 재접속 시 서버가 다시 가져다준다 (유실 없음)
                        state.on_disconnect();
                        state.inbox.clear();
                        state.queued_ids.clear();
                        tracing::info!("idle — parking connection (messages queue server-side)");
                        state.set_state(ClientState::Parked);
                        match cmds.recv().await {
                            Some(cmd) => resume_cmd = Some(cmd), // 재접속 직후 처리
                            None => {
                                state.set_state(dropped); // 핸들 전부 드롭 — 종료
                                return;
                            }
                        }
                        tracing::info!("command arrived — resuming from park");
                        continue; // 백오프 없이 즉시 재접속
                    }
                    ConnEnd::Closed => {
                        state.set_state(dropped);
                        return;
                    }
                }
            }
            Err(e) => {
                let msg = e.to_string();
                if let Some(reason) = msg.strip_prefix("FATAL: ") {
                    // JOIN이 비재시도 오류로 거부됨 (무효 토큰·grant 없음 등)
                    let Some(reload) = state.opts.token_reload.clone() else {
                        // 대화형 어댑터: 재시도 무의미 — 호출자에게 오류로 정직하게, 종료
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
                        state.set_state(ClientState::Stopped {
                            reason: reason.to_owned(),
                        });
                        return;
                    };
                    // 데몬 모드 (2026-09-02, 맥북 실사고 "살아 있는 채 영구 정지"의 근본 수정):
                    // 죽지 않고 정지 상태로 물러나 긴 백오프로 재시도하며 매번 저장소의 토큰을
                    // 다시 읽는다 — 같은 머신에서 재enroll(토큰 회전)하면 재기동 없이 복구된다.
                    // 종료 대신 정지인 이유: 재기동 감독이 없는 상주(작업 스케줄러 등)에서도
                    // 동작해야 하고, 무효 토큰으로 감독자가 재기동을 반복하는 소음도 피한다
                    fatal_attempt += 1;
                    let wait = fatal_backoff(state.opts.fatal_retry_base, fatal_attempt);
                    tracing::error!(
                        %reason,
                        retry_in_s = wait.as_secs(),
                        "join rejected — suspended; will re-read the token and retry"
                    );
                    state.set_state(ClientState::Suspended {
                        reason: reason.to_owned(),
                        retry_in_s: wait.as_secs(),
                    });
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        cmd = cmds.recv() => match cmd {
                            Some(cmd) => resume_cmd = Some(cmd), // 재접속 직후 처리
                            None => {
                                state.set_state(dropped);
                                return;
                            }
                        },
                    }
                    if let Some(fresh) = (reload.0)()
                        && fresh != state.opts.token
                    {
                        tracing::info!("token on disk changed — retrying with the new token");
                        state.opts.token = fresh;
                        fatal_attempt = 0;
                    }
                    continue;
                }
                tracing::warn!(error = %e, attempt, "connect failed");
                state.set_state(ClientState::Reconnecting { attempt });
            }
        }
        attempt += 1;
        tokio::time::sleep(backoff(attempt)).await;
    }
}

/// 정지 재시도 간격 — base × (1, 2, 4, 10, 20, 30) 상한 30배 (기본 30s → 15분 상한).
fn fatal_backoff(base: Duration, attempt: u32) -> Duration {
    const STEPS: [u32; 6] = [1, 2, 4, 10, 20, 30];
    base * STEPS[(attempt.saturating_sub(1) as usize).min(STEPS.len() - 1)]
}

// 치명(JOIN 비재시도 거부)은 connect_and_join의 "FATAL: " 오류 경로가 담당한다
enum ConnEnd {
    Reconnect,
    /// 다른 세션이 자리를 가져감 (`agent/session-conflict` 수신, 2.2).
    TakenOver,
    /// 유휴 파킹 (idle_park 초과) — 접속을 내려놓고 다음 명령까지 대기 (2026-09-01).
    Park,
    Closed,
}

/// BLOB_PUT (PROTOCOL 3.2·5.2) — claim-check 업로드. 반환된 참조를 엔벨로프에 싣는다.
pub async fn upload_blob(
    server: &str,
    channel: &str,
    token: &str,
    content_type: &str,
    bytes: Vec<u8>,
) -> anyhow::Result<brevduva_protocol::PayloadRef> {
    use anyhow::Context as _;
    let resp = reqwest::Client::new()
        .post(format!(
            "{}/v1/blobs?channel={channel}",
            server.trim_end_matches('/')
        ))
        .bearer_auth(token)
        .header("content-type", content_type)
        .body(bytes)
        .send()
        .await
        .context("server unreachable")?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    anyhow::ensure!(
        status.is_success(),
        "blob upload rejected: {}",
        body["message"].as_str().unwrap_or(status.as_str())
    );
    Ok(brevduva_protocol::PayloadRef {
        id: body["id"].as_str().unwrap_or_default().to_owned(),
        size: body["size"].as_u64().unwrap_or_default(),
        sha256: body["sha256"].as_str().unwrap_or_default().to_owned(),
        content_type: body["content_type"].as_str().unwrap_or_default().to_owned(),
    })
}

/// BLOB_GET (5.2, Range 지원) — claim-check 다운로드. range = (start, inclusive_end?).
/// 점진 읽기(소형 컨텍스트 보호)의 재료: 어댑터는 머리부터 잘라 읽는다.
pub async fn download_blob(
    server: &str,
    channel: &str,
    token: &str,
    id: &str,
    range: Option<(u64, Option<u64>)>,
) -> anyhow::Result<Vec<u8>> {
    use anyhow::Context as _;
    let mut req = reqwest::Client::new()
        .get(format!(
            "{}/v1/blobs/{id}?channel={channel}",
            server.trim_end_matches('/')
        ))
        .bearer_auth(token);
    if let Some((start, end)) = range {
        let end = end.map(|e| e.to_string()).unwrap_or_default();
        req = req.header("range", format!("bytes={start}-{end}"));
    }
    let resp = req.send().await.context("server unreachable")?;
    let status = resp.status();
    if !status.is_success() {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        anyhow::bail!(
            "blob download rejected: {}",
            body["message"].as_str().unwrap_or(status.as_str())
        );
    }
    Ok(resp.bytes().await.context("blob body")?.to_vec())
}

/// 롱폴 엔드포인트(7장 — WS와 동일 시맨틱)로 프레임 하나를 보내고 OK 본문을 받는다.
/// **액터를 거치지 않는다**: 데몬이 자리를 양보한 standby 중에도 즉시 동작해야 하는 조회·발행용
/// (2026-09-04 — `client.fetch()`는 standby 중 큐에 갇혀 최대 `standby_probe`만큼 늦는다).
/// JOIN도 하지 않아 세션·프레즌스를 건드리지 않는다.
async fn http_frame(
    server: &str,
    channel: &str,
    token: &str,
    op: ClientOp,
) -> anyhow::Result<OkBody> {
    use anyhow::Context as _;
    let resp = reqwest::Client::new()
        .post(format!(
            "{}/v1/frames?channel={}",
            server.trim_end_matches('/'),
            channel
        ))
        .bearer_auth(token)
        .json(&ClientFrame {
            seq: Some(1),
            re: None,
            op,
        })
        .send()
        .await
        .context("server unreachable")?;
    let frame: ServerFrame = resp.json().await.context("malformed server frame")?;
    match frame.op {
        ServerOp::Ok(body) => Ok(body),
        ServerOp::Err(e) => anyhow::bail!("{}", e.message),
        other => anyhow::bail!("unexpected server op: {other:?}"),
    }
}

/// 이력 조회 (FETCH) — JOIN 없는 읽기. standby와 무관하게 즉시 답한다.
pub async fn fetch_history(
    server: &str,
    channel: &str,
    token: &str,
    after_id: Option<&str>,
    limit: u32,
) -> anyhow::Result<Vec<Envelope>> {
    let op = ClientOp::Fetch {
        topics: None,
        after_id: after_id.and_then(|s| MessageId::parse(s).ok()),
        after_ts: None,
        before_id: None,
        newest_first: false,
        limit: Some(limit),
    };
    Ok(http_frame(server, channel, token, op)
        .await?
        .messages
        .unwrap_or_default())
}

/// 발행 (PUB) — 액터 밖. 데몬이 깨운 세션을 대신해 보고할 때 쓴다: 세션이 자리를 가져간
/// standby 중에도 즉시 나가야 하고, 자리 다툼을 일으켜서도 안 된다 (2026-09-04).
pub async fn publish_direct(
    server: &str,
    channel: &str,
    token: &str,
    agent: &str,
    spec: &PublishSpec,
) -> anyhow::Result<MessageId> {
    let env = Envelope {
        v: brevduva_protocol::PROTOCOL_VERSION,
        id: None,
        ts: None,
        client_key: ClientKey::generate(),
        from: Ident::parse(agent)?,
        to: Address::parse(&spec.to)?,
        kind: spec.kind,
        correlation_id: match &spec.correlation_id {
            Some(c) => Some(MessageId::parse(c)?),
            None => None,
        },
        expects: spec.expects,
        ttl_ms: spec.ttl_ms,
        hops: spec.hops,
        content_type: spec.content_type.clone(),
        payload: spec.payload.clone(),
        payload_ref: spec.payload_ref.clone(),
        meta: spec.meta.clone(),
    };
    http_frame(server, channel, token, ClientOp::Pub(env))
        .await?
        .id
        .ok_or_else(|| anyhow::anyhow!("server accepted the message but returned no id"))
}

/// 채널 발견 (PROTOCOL 10.2) — 토큰으로 자신의 grant 채널 조회. JOIN 불요라
/// 세션·프레즌스에 영향이 없다. 반환: (org, agent, channels).
pub async fn discover_channels(
    server: &str,
    token: &str,
) -> anyhow::Result<(String, String, Vec<String>)> {
    use anyhow::Context as _;
    let resp = reqwest::Client::new()
        .get(format!(
            "{}/v1/agent/channels",
            server.trim_end_matches('/')
        ))
        .bearer_auth(token)
        .send()
        .await
        .context("server unreachable")?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    anyhow::ensure!(
        status.is_success(),
        "channel discovery rejected: {}",
        body["message"].as_str().unwrap_or(status.as_str())
    );
    let get = |k: &str| body[k].as_str().unwrap_or_default().to_owned();
    let channels = body["channels"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Ok((get("org"), get("agent"), channels))
}

/// standby: long-poll PRESENCE 프로브(세션·큐 무접촉)로 자리가 빌 때까지 대기.
/// 오류(서버 불가 등)는 "자리 비어 있음"으로 간주 — 재접속 경로의 백오프가 뒷일을 맡는다.
/// standby 대기 — true = 자리가 비었다(재JOIN), false = 핸들이 전부 드롭됐다(종료).
async fn standby_until_free(opts: &ClientOptions, cmds: &mpsc::Receiver<Cmd>) -> bool {
    let http = reqwest::Client::new();
    loop {
        // 핸들 전부 드롭(데몬 pause·바인딩 이탈)이면 프로브를 기다리지 않고 물러난다 (2026-09-03
        // lucadm 실측): 종전엔 자리가 빌 때까지 기다렸다 재JOIN한 뒤에야 닫힘을 알아채 — 정지
        // 중 유령 JOIN(메시지 수신·JOIN 한도 소모). 1초 조각 수면으로 닫힘을 살핀다
        let deadline = Instant::now() + opts.standby_probe;
        loop {
            if cmds.is_closed() {
                return false;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            tokio::time::sleep(left.min(Duration::from_secs(1))).await;
        }
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
            return true;
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
    resume_cmd: Option<Cmd>,
    cmds: &mut mpsc::Receiver<Cmd>,
) -> ConnEnd {
    for frame in backlog {
        if let ServerOp::Deliver(env) = frame.op
            && let Some(seq) = frame.seq
        {
            state.handle_deliver(&mut ws, seq, *env).await;
        }
    }
    // 파킹을 깨운 명령을 접속 직후 처리 (2026-09-01)
    if let Some(cmd) = resume_cmd {
        state.handle_cmd(&mut ws, cmd).await;
    }
    // 유휴 파킹 마감 — 명령마다 뒤로 밀리고, 도달 시점에 일이 남아 있으면 한 창 더 미룬다.
    // 핑 틱에 얹지 않고 전용 타이머인 이유: 판정 시점이 창 폭보다 굵어지면(20s 틱) 방치
    // 창이 격리 예산과 경합한다 — 파킹은 격리보다 확실히 먼저 일어나야 한다 (2026-09-01)
    let mut park_at = state.opts.idle_park.map(|d| Instant::now() + d);
    let mut ping_tick = interval(Duration::from_secs(20));
    ping_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            cmd = cmds.recv() => match cmd {
                Some(cmd) => {
                    if let Some(d) = state.opts.idle_park {
                        park_at = Some(Instant::now() + d);
                    }
                    state.handle_cmd(&mut ws, cmd).await;
                }
                None => return ConnEnd::Closed, // 핸들 전부 드롭 — 종료
            },
            _ = tokio::time::sleep_until(park_at.unwrap_or_else(Instant::now)), if park_at.is_some() => {
                // 유휴 파킹 판정 (2026-09-01): 명령이 끊긴 지 idle_park 초과 + 진행 중인 일 없음.
                // unacked(확인 유보 = 처리 실패 신호)가 남아 있으면 파킹하지 않는다 —
                // 그 재전달→포이즌 가시화는 페이즈 20의 의도된 실패 표면
                state.waiters.retain(|(_, _, tx)| !tx.is_closed());
                if state.waiters.is_empty()
                    && state.pending.is_empty()
                    && state.outbox.is_empty()
                    && state.unacked.is_empty()
                {
                    let _ = ws.close(None).await;
                    return ConnEnd::Park;
                }
                // 일이 진행 중 — 한 창 더 뒤로 (대기자 홀드 등은 명령 없이도 끝나므로 재확인)
                park_at = state.opts.idle_park.map(|d| Instant::now() + d);
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

    /// 2026-09-03 (lucadm 실측): 핸들이 전부 드롭된 액터는 standby 프로브 주기를 기다리지 않고
    /// 즉시 물러난다 — 데몬 pause가 자리를 비운 뒤 유령 재JOIN이 없어야 한다
    #[tokio::test]
    async fn standby_wait_ends_when_handles_drop() {
        let mut opts = ClientOptions::new("http://127.0.0.1:9", "ch", "agent", "brv_x");
        opts.standby_probe = Duration::from_secs(600);
        let (tx, rx) = mpsc::channel::<Cmd>(1);
        drop(tx);
        let freed = timeout(Duration::from_secs(5), standby_until_free(&opts, &rx))
            .await
            .expect("must not wait for the probe interval");
        assert!(!freed, "dropped handles mean stop, not rejoin");
    }

    /// 2026-09-02: 정지 재시도 간격은 단조 증가 후 상한 — 무효 토큰으로 서버를 두드리지 않으면서
    /// 재enroll 후 15분 안에는 반드시 다시 시도한다
    #[test]
    fn fatal_backoff_grows_then_caps() {
        let base = Duration::from_secs(30);
        let seq: Vec<u64> = (1..=8).map(|n| fatal_backoff(base, n).as_secs()).collect();
        assert_eq!(seq, vec![30, 60, 120, 300, 600, 900, 900, 900]);
        assert_eq!(fatal_backoff(base, 0).as_secs(), 30);
    }

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
