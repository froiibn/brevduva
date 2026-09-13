// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! 로컬 세션 등록부 — "이 머신의 어느 세션이 어느 바인딩을 쥐고 있는가"의 진실
//! (RECEIVER_DESIGN.md P4·P7).
//!
//! 서버의 단일 활성 수신 세션(PROTOCOL 2.2)과 **같은 규칙을 로컬로 내린 것**이다. 종전에는
//! 세션마다 서버에 JOIN해 서버가 자리 다툼을 중재했고, 그 부작용이 데몬 standby 왕복·설치기
//! 재기동 시 라이브 세션 밀어내기였다. 이제 서버에 붙는 것은 리시버 하나(P2)이고, 세션 사이의
//! 자리 다툼은 여기서 끝난다.
//!
//! **전송 수단을 모른다** (로봇 로드맵 — REBUILD_PLAN §1): 등록부는 `Envelope`와 세션의 능력
//! 선언만 안다. MCP/HTTP든 유닉스 소켓이든 시리얼이든 `Sink` 하나로 꽂힌다.
//!
//! 규칙 요약
//! - 바인딩당 홀더는 **하나**. 다른 세션이 `become`하면 기존 홀더는 밀려나고 통지받는다(P7).
//! - 단 **hold**(작업 중 잠금)가 열려 있으면 `become`은 거부된다 — DB 잠금과 같은 의미론:
//!   획득 = request 수락, 해제 = 최종 reply/report 확정 또는 세션 사망(2026-09-09 U2·U6).
//! - 홀더라도 **러너 입력 통로가 실제로 붙지 않았으면** 라우팅상 "받을 수 있는" 세션이 아니다
//!   (P4·U3, 2026-09-10 정정 — 전송로만 열린 세션은 아니다). 그런 바인딩의 메시지는 무인 깨우기로
//!   간다(P5).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use brevduva_protocol::{ClientKey, Envelope, ReceiveMode};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::sync::mpsc;

/// 리시버가 발급하는 로컬 세션 식별자.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SessionId(String);

impl SessionId {
    pub fn generate() -> Self {
        Self(ClientKey::generate().to_string())
    }

    /// 전송 계층이 헤더에서 읽은 값. 모르는 값이면 조회에서 걸러지므로 형식 검증은 하지 않는다.
    pub fn parse(raw: &str) -> Self {
        Self(raw.to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 깨우기 1회의 식별자 — 스폰과 그 세션의 부착을 잇는다(`BREVDUVA_WAKE`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WakeId(String);

impl WakeId {
    pub fn generate() -> Self {
        Self(ClientKey::generate().to_string())
    }

    pub fn parse(raw: &str) -> Self {
        Self(raw.to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WakeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 바인딩 키 — `org/agent@channel` (`Binding::full_label`). 조직 간 동명 구분 포함.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BindingKey(String);

impl BindingKey {
    pub fn of(binding: &crate::config::Binding) -> Self {
        Self(binding.full_label())
    }

    pub fn parse(raw: &str) -> Self {
        Self(raw.to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BindingKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 세션이 어떻게 생겼는가 — 로컬 정책 판단의 근거(REBUILD_PLAN §1.5).
///
/// 종전에는 `BREVDUVA_BINDING` 환경변수의 존재로 판정했다. 그 값은 깨어난 세션이 자식
/// 프로세스에 물려주거나 지울 수 있어 판정이 흔들렸다. 이제 **리시버가 자기가 깨운 것을
/// 기억**하므로 세션의 주장과 무관하게 확정된다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum Origin {
    /// 사람이 연 세션 — 리시버 관리 도구가 보인다.
    Attended,
    /// 리시버가 깨운 세션 — 로컬 정책 변경이 거부된다(2026-09-03 결정 유지).
    Woken { wake: WakeId },
}

impl Origin {
    pub fn is_attended(&self) -> bool {
        matches!(self, Origin::Attended)
    }
}

/// 세션 능력 선언 — PROTOCOL 4장 `Capabilities`와 같은 모양을 로컬에서 재사용한다.
/// 로봇 단계의 `cbor`·바이너리 페이로드가 필드 신설 없이 들어오도록 자리를 미리 둔다.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionCapabilities {
    /// 수신 방식 선언. 선언일 뿐이다 — "받을 수 있음"(P4)은 러너 입력 통로(`DeliveryTarget`)가
    /// 실제로 붙었는가로 정한다(2026-09-10 정정).
    #[serde(default)]
    pub modes: Vec<ReceiveMode>,
    #[serde(default)]
    pub content_types: Vec<String>,
    #[serde(default)]
    pub encodings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_inline_bytes: Option<u64>,
    /// 확장 슬롯 — 프런트엔드·로봇 메타데이터.
    #[serde(default)]
    pub meta: Map<String, Value>,
}

/// 세션이 붙을 때 주는 것.
#[derive(Debug, Clone)]
pub struct AttachSpec {
    /// 러너·제어기 id (`claude`, `codex`, 로봇이면 그 제어기 이름). 추측하지 않는다 — 모르면 None.
    pub host: Option<String>,
    pub origin: Origin,
    pub capabilities: SessionCapabilities,
    /// 사람이 읽는 설명 — `brv status`와 관리 도구에 그대로 나온다.
    pub description: Option<String>,
}

/// 리시버 → 세션 밀어넣기. 전송 수단은 프런트엔드가 감춘다(로봇 로드맵).
pub type Sink = mpsc::Sender<PushEvent>;

/// 세션에게 밀어 넣는 사건.
#[derive(Debug, Clone)]
pub enum PushEvent {
    /// 수신 메시지. 세션이 `receipt`로 수락을 확인해야 리시버가 서버에 ACK한다(P4·P6).
    Message {
        binding: BindingKey,
        envelope: Box<Envelope>,
        /// 수락 확인용 1회용 표. 위조 방지 겸 어느 전달의 확인인지 식별한다.
        receipt: String,
    },
    /// 이 세션이 해당 바인딩에서 밀려났다(P7). 이후 그 바인딩 도구 호출은 거부된다.
    Evicted {
        binding: BindingKey,
        /// 밀어낸 쪽의 host(알면) — 사람이 상황을 이해할 수 있게.
        by: Option<String>,
    },
    /// Claude Code Channels 사건 — 모델 턴을 여는 채널 알림(7c). `content`는 고정 안내뿐이고 동료
    /// 본문은 싣지 않는다(본문은 receipt 도구 결과로만). `meta` 키는 영문·숫자·밑줄만 쓴다.
    ChannelEvent {
        content: String,
        meta: Map<String, Value>,
    },
    /// 리시버가 내려간다 — 세션은 정리하고 빠진다.
    Shutdown,
}

/// 작업 중 잠금 — DB 잠금과 같은 의미론(2026-09-09 U2·U6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hold {
    /// 이 잠금이 걸린 작업 — 원 메시지 id(회신의 `correlation_id`).
    pub correlation_id: String,
    pub owner: HoldOwner,
    pub since_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "owner", rename_all = "snake_case")]
pub enum HoldOwner {
    Session {
        session: SessionId,
    },
    /// 깨우기를 스폰했지만 그 세션이 아직 붙지 않은 구간을 보호한다 — 이 창이 열려 있는 동안
    /// 다른 세션이 끼어들면 깨어난 세션이 자기 작업을 시작하자마자 밀려난다.
    PendingWake {
        wake: WakeId,
    },
}

impl HoldOwner {
    fn belongs_to(&self, session: &Session) -> bool {
        match self {
            HoldOwner::Session { session: id } => id == &session.id,
            HoldOwner::PendingWake { wake } => match &session.origin {
                Origin::Woken { wake: own } => own == wake,
                Origin::Attended => false,
            },
        }
    }
}

/// 모델 턴을 여는 러너 입력 통로의 종류 (P4). 등록부는 이름과 통로(`Sink`)만 안다 — 그 통로가
/// 무엇으로 이어지는지는 평면이 안다. Monitor 스트림·Codex queue·Channels 알림·로봇 제어기가 같은
/// 자리에 꽂힌다(REBUILD_PLAN §1.3). 구현된 종류만 둔다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    /// Claude Code의 네이티브 Monitor — 리시버가 연 루프백 스트림을 모델이 구독한다.
    Monitor,
    /// Codex CLI 작업의 네이티브 대기열 — 리시버가 사용자 명의로 `codex queue`에 넣는다(7b).
    CodexQueue,
    /// Claude Code Channels — 브리지의 stdio로 채널 사건을 싣는다. 확인 사건의 receipt로 준비된다(7c).
    Channels,
    /// Codex Desktop 앱 안의 작업 — 리시버가 사용자 명의 도우미로 그 작업의 소유자에게 턴을 연다(7d).
    CodexDesktop,
}

impl TargetKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetKind::Monitor => "monitor",
            TargetKind::CodexQueue => "codex_queue",
            TargetKind::Channels => "claude_channel",
            TargetKind::CodexDesktop => "codex_desktop",
        }
    }
}

/// 세션의 러너 입력 통로. `ready`가 참이 된 뒤에야 그 세션은 수신자다 — 통로를 만들었다는
/// 사실이 아니라 실제로 붙었다는 사실이 기준이다(2026-09-10 P4 정정).
#[derive(Debug)]
pub struct DeliveryTarget {
    /// 통로 세대 — 교체된 옛 통로의 소실 통지가 새 통로를 지우지 않게 대조한다.
    pub id: String,
    pub kind: TargetKind,
    pub ready: bool,
    sink: Sink,
}

impl DeliveryTarget {
    pub fn new(kind: TargetKind, sink: Sink) -> Self {
        Self {
            id: ClientKey::generate().to_string(),
            kind,
            ready: false,
            sink,
        }
    }
}

/// 등록된 세션.
#[derive(Debug)]
pub struct Session {
    pub id: SessionId,
    pub host: Option<String>,
    pub origin: Origin,
    pub capabilities: SessionCapabilities,
    pub description: Option<String>,
    pub attached_unix: u64,
    pub bindings: BTreeSet<BindingKey>,
    /// 제어 통로 — 밀려남·종료 통지. 이 통로가 열려 있어도 수신자는 아니다(P4).
    sink: Sink,
    /// 러너 입력 통로 — 붙어 있을 때만 수신자다.
    target: Option<DeliveryTarget>,
}

impl Session {
    /// P4: 모델 턴을 여는 러너 입력 통로가 실제로 붙어 있는가. 전송로(SSE 등)만 열린 세션은
    /// 아니다 — 일반 MCP 호스트는 알림으로 모델 턴을 열지 않는다(2026-09-10 정정).
    pub fn can_receive(&self) -> bool {
        self.target
            .as_ref()
            .is_some_and(|target| target.ready && !target.sink.is_closed())
    }

    pub fn target(&self) -> Option<&DeliveryTarget> {
        self.target.as_ref()
    }

    /// 제어 통지(밀려남·종료). 실패는 이미 사라진 세션이라는 뜻이다.
    pub fn push(&self, event: PushEvent) -> Result<(), PushError> {
        send(&self.sink, event)
    }

    /// 러너 입력 통로로 넘긴다. 붙지 않은 통로에는 넘기지 않는다 — 넘길 곳이 없는 것과 같다.
    /// 실패는 "넘기지 못함"이고, P6에 따라 메시지는 서버 큐에 남는다(확정하지 않는다).
    pub fn deliver(&self, event: PushEvent) -> Result<(), PushError> {
        match &self.target {
            Some(target) if target.ready => send(&target.sink, event),
            _ => Err(PushError::NoTarget),
        }
    }
}

fn send(sink: &Sink, event: PushEvent) -> Result<(), PushError> {
    sink.try_send(event).map_err(|e| match e {
        mpsc::error::TrySendError::Full(_) => PushError::Busy,
        mpsc::error::TrySendError::Closed(_) => PushError::Gone,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushError {
    /// 통로가 아직 앞의 것을 소화하지 못했다 — 붙어 있지만 지금 못 받음(P6: 큐 대기).
    Busy,
    /// 통로가 닫혔다 — 세션이나 통로가 사라졌다.
    Gone,
    /// 러너 입력 통로가 없거나 아직 붙지 않았다 — 라우팅상 붙은 세션이 아니다(P4).
    NoTarget,
}

impl std::fmt::Display for PushError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PushError::Busy => f.write_str("session is not taking deliveries right now"),
            PushError::Gone => f.write_str("session is gone"),
            PushError::NoTarget => f.write_str("session has no attached input path"),
        }
    }
}

/// `become` 결과.
#[derive(Debug, PartialEq, Eq)]
pub enum Became {
    /// 이미 이 세션이 홀더였다.
    Already,
    /// 홀더가 됐다. 밀어낸 세션이 있으면 그 id (이미 통지는 보냈다).
    Took { evicted: Option<SessionId> },
}

/// `become` 거부 — 잠긴 바인딩.
#[derive(Debug, PartialEq, Eq)]
pub struct Held {
    pub binding: BindingKey,
    pub holder: Option<SessionId>,
    pub host: Option<String>,
    /// 열려 있는 작업들.
    pub open: Vec<String>,
}

impl std::fmt::Display for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} is held by a session working on {} ({}); it is released when that work's final reply or report is confirmed",
            self.binding,
            self.open.join(", "),
            self.host.as_deref().unwrap_or("unknown runner"),
        )
    }
}

impl std::error::Error for Held {}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// 로컬 세션 등록부.
#[derive(Debug, Default)]
pub struct Registry {
    sessions: HashMap<SessionId, Session>,
    /// 바인딩 → 현재 홀더.
    holders: HashMap<BindingKey, SessionId>,
    /// 바인딩 → 열린 잠금들 (correlation_id 순).
    holds: HashMap<BindingKey, BTreeMap<String, Hold>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    // ---------------------------------------------------------------- 세션 수명

    /// 세션 등록. 이 시점에는 아무 바인딩도 쥐지 않는다 — `become`이 정체성을 정한다(P3).
    pub fn attach(&mut self, spec: AttachSpec, sink: Sink) -> SessionId {
        let id = SessionId::generate();
        self.sessions.insert(
            id.clone(),
            Session {
                id: id.clone(),
                host: spec.host,
                origin: spec.origin,
                capabilities: spec.capabilities,
                description: spec.description,
                attached_unix: now_unix(),
                bindings: BTreeSet::new(),
                sink,
                target: None,
            },
        );
        id
    }

    /// 세션 종료 — 쥔 바인딩과 **열린 잠금을 전부 놓는다**(U2: 세션 사망 = 잠금 해제, 롤백은
    /// 어댑터의 failed report 대발행이 맡는다). 놓인 바인딩을 돌려주므로 라우터가 재평가한다.
    pub fn detach(&mut self, id: &SessionId) -> Vec<BindingKey> {
        let Some(session) = self.sessions.remove(id) else {
            return Vec::new();
        };
        let mut freed = Vec::new();
        for binding in session.bindings {
            if self.holders.get(&binding) == Some(id) {
                self.holders.remove(&binding);
            }
            if let Some(open) = self.holds.get_mut(&binding) {
                open.retain(|_, hold| !matches!(&hold.owner, HoldOwner::Session { session } if session == id));
                if open.is_empty() {
                    self.holds.remove(&binding);
                }
            }
            freed.push(binding);
        }
        freed
    }

    pub fn session(&self, id: &SessionId) -> Option<&Session> {
        self.sessions.get(id)
    }

    /// 러너 입력 통로를 단다(아직 준비 전). 이미 있으면 바꾼다 — 옛 통로의 송신단이 사라져 그
    /// 통로의 작업이 끝나고, 그쪽 소실 통지는 세대가 달라 무시된다.
    pub fn set_target(&mut self, id: &SessionId, target: DeliveryTarget) -> bool {
        let Some(session) = self.sessions.get_mut(id) else {
            return false;
        };
        session.target = Some(target);
        true
    }

    /// 통로가 실제로 붙었다(또는 끊겼다). 세대가 맞을 때만 반영한다.
    pub fn set_target_ready(&mut self, id: &SessionId, target: &str, ready: bool) -> bool {
        match self.sessions.get_mut(id).and_then(|s| s.target.as_mut()) {
            Some(current) if current.id == target => {
                current.ready = ready;
                true
            }
            _ => false,
        }
    }

    /// 통로를 뗀다. 세대가 맞을 때만 — 이미 새 통로로 바뀌었으면 건드리지 않는다.
    pub fn clear_target(&mut self, id: &SessionId, target: &str) -> bool {
        let Some(session) = self.sessions.get_mut(id) else {
            return false;
        };
        if session.target.as_ref().is_some_and(|t| t.id == target) {
            session.target = None;
            true
        } else {
            false
        }
    }

    pub fn sessions(&self) -> impl Iterator<Item = &Session> {
        self.sessions.values()
    }

    /// 세션의 정체를 확정한다 — 리시버가 깨운 것으로 확인됐을 때만 승격시킨다. 세션이 스스로
    /// 주장해서 되는 것이 아니라, 리시버가 발급한 깨우기 식별자와 맞아야 호출된다.
    pub fn set_origin(&mut self, id: &SessionId, origin: Origin) {
        if let Some(session) = self.sessions.get_mut(id) {
            session.origin = origin;
        }
    }

    // ---------------------------------------------------------------- 정체성 (P7)

    /// 이 세션이 바인딩을 쥔다. 최신이 이기되, 잠긴 바인딩은 거부한다.
    pub fn become_binding(
        &mut self,
        id: &SessionId,
        binding: &BindingKey,
    ) -> anyhow::Result<Became> {
        anyhow::ensure!(self.sessions.contains_key(id), "unknown local session");
        if self.holders.get(binding) == Some(id) {
            return Ok(Became::Already);
        }
        // 잠긴 바인딩: 자기 것(깨우기 창)이 아니면 거부 — 작업 중 세션을 지킨다.
        if let Some(open) = self.holds.get(binding).filter(|h| !h.is_empty()) {
            let session = self.sessions.get(id).expect("checked above");
            if !open.values().all(|hold| hold.owner.belongs_to(session)) {
                let holder = self.holders.get(binding).cloned();
                return Err(Held {
                    binding: binding.clone(),
                    host: holder
                        .as_ref()
                        .and_then(|h| self.sessions.get(h))
                        .and_then(|s| s.host.clone()),
                    holder,
                    open: open.keys().cloned().collect(),
                }
                .into());
            }
            // 자기 깨우기의 보호 창 — 세션 소유로 승계한다.
            let owner = HoldOwner::Session {
                session: id.clone(),
            };
            for hold in self
                .holds
                .get_mut(binding)
                .expect("checked above")
                .values_mut()
            {
                hold.owner = owner.clone();
            }
        }
        let taking_host = self.sessions.get(id).and_then(|s| s.host.clone());
        let evicted = match self.holders.insert(binding.clone(), id.clone()) {
            Some(previous) if &previous != id => {
                if let Some(session) = self.sessions.get_mut(&previous) {
                    session.bindings.remove(binding);
                    // 통지 실패는 무시한다 — 이미 사라진 세션에게 알릴 것은 없다.
                    let _ = session.push(PushEvent::Evicted {
                        binding: binding.clone(),
                        by: taking_host,
                    });
                }
                Some(previous)
            }
            _ => None,
        };
        self.sessions
            .get_mut(id)
            .expect("checked above")
            .bindings
            .insert(binding.clone());
        Ok(Became::Took { evicted })
    }

    /// 이 세션이 바인딩을 자발적으로 놓는다. 잠금이 열려 있으면 거부한다.
    pub fn release_binding(&mut self, id: &SessionId, binding: &BindingKey) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.holders.get(binding) == Some(id),
            "this session does not hold {binding}"
        );
        if let Some(open) = self.holds.get(binding).filter(|h| !h.is_empty()) {
            return Err(Held {
                binding: binding.clone(),
                holder: Some(id.clone()),
                host: self.sessions.get(id).and_then(|s| s.host.clone()),
                open: open.keys().cloned().collect(),
            }
            .into());
        }
        self.holders.remove(binding);
        if let Some(session) = self.sessions.get_mut(id) {
            session.bindings.remove(binding);
        }
        Ok(())
    }

    pub fn holder(&self, binding: &BindingKey) -> Option<&Session> {
        self.holders
            .get(binding)
            .and_then(|id| self.sessions.get(id))
    }

    /// P5의 "붙은 유인 세션" — 홀더이면서 러너 입력 통로가 실제로 붙은 세션만. 홀더가 있어도
    /// 통로가 없으면(전송로만 열린 세션, U3의 GUI 등) None이고, 라우터는 무인 깨우기로 간다.
    pub fn receiver_for(&self, binding: &BindingKey) -> Option<&Session> {
        self.holder(binding).filter(|s| s.can_receive())
    }

    /// 이 세션이 그 바인딩으로 발행할 자격이 있는가 — 홀더만 자기 정체성으로 말한다.
    pub fn may_act_as(&self, id: &SessionId, binding: &BindingKey) -> bool {
        self.holders.get(binding) == Some(id)
    }

    // ---------------------------------------------------------------- 잠금 (P7 hold)

    /// 깨우기 스폰 직후 — 세션이 붙기 전 창을 보호한다. 붙으면 `become_binding`이 승계한다.
    pub fn hold_for_wake(
        &mut self,
        binding: &BindingKey,
        wake: &WakeId,
        correlation_ids: &[String],
    ) {
        let open = self.holds.entry(binding.clone()).or_default();
        for correlation_id in correlation_ids {
            open.entry(correlation_id.clone()).or_insert_with(|| Hold {
                correlation_id: correlation_id.clone(),
                owner: HoldOwner::PendingWake { wake: wake.clone() },
                since_unix: now_unix(),
            });
        }
    }

    /// 깨우기가 끝났는데 세션이 끝내 붙지 않았다 — 그 창의 보호를 거둔다.
    pub fn release_wake(&mut self, wake: &WakeId) -> Vec<BindingKey> {
        let mut freed = Vec::new();
        self.holds.retain(|binding, open| {
            let before = open.len();
            open.retain(
                |_, hold| !matches!(&hold.owner, HoldOwner::PendingWake { wake: w } if w == wake),
            );
            if open.len() != before {
                freed.push(binding.clone());
            }
            !open.is_empty()
        });
        freed
    }

    /// 작업 시작 — 세션이 회신을 요구하는 메시지를 수락했다(트랜잭션 시작).
    pub fn hold_acquire(
        &mut self,
        id: &SessionId,
        binding: &BindingKey,
        correlation_id: &str,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.holders.get(binding) == Some(id),
            "only the session holding {binding} can take work on it"
        );
        self.holds
            .entry(binding.clone())
            .or_default()
            .entry(correlation_id.to_owned())
            .or_insert_with(|| Hold {
                correlation_id: correlation_id.to_owned(),
                owner: HoldOwner::Session {
                    session: id.clone(),
                },
                since_unix: now_unix(),
            });
        Ok(())
    }

    /// 작업 종료 — 최종 reply/report가 확정됐다(커밋). 진행 보고는 여기 오지 않는다.
    /// 잠금이 없던 작업이면 조용히 참을 돌려준다 — 회신은 잠금과 무관하게 유효하다.
    pub fn hold_release(&mut self, binding: &BindingKey, correlation_id: &str) -> bool {
        let Some(open) = self.holds.get_mut(binding) else {
            return false;
        };
        let removed = open.remove(correlation_id).is_some();
        if open.is_empty() {
            self.holds.remove(binding);
        }
        removed
    }

    /// 소유자의 강제 해제 — 살아 있으면서 회신하지 않는 세션(긴 트랜잭션) 대비.
    /// DB에서 세션을 강제 종료하는 것과 같은 자리이며, 사람이 명시적으로 부른다.
    pub fn force_release(&mut self, binding: &BindingKey) -> Vec<String> {
        self.holds
            .remove(binding)
            .map(|open| open.into_keys().collect())
            .unwrap_or_default()
    }

    pub fn holds(&self, binding: &BindingKey) -> Vec<&Hold> {
        self.holds
            .get(binding)
            .map(|open| open.values().collect())
            .unwrap_or_default()
    }

    pub fn is_held(&self, binding: &BindingKey) -> bool {
        self.holds.get(binding).is_some_and(|h| !h.is_empty())
    }

    /// 리시버 종료 — 붙은 세션 전부에게 알린다.
    pub fn shutdown(&self) {
        for session in self.sessions.values() {
            let _ = session.push(PushEvent::Shutdown);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(label: &str) -> BindingKey {
        BindingKey::parse(label)
    }

    fn spec(host: &str, origin: Origin) -> AttachSpec {
        AttachSpec {
            host: Some(host.to_owned()),
            origin,
            capabilities: SessionCapabilities::default(),
            description: None,
        }
    }

    /// 세션 하나를 붙이고 제어 통로를 함께 돌려준다. 러너 입력 통로는 붙이지 않는다.
    fn attach(registry: &mut Registry, host: &str) -> (SessionId, mpsc::Receiver<PushEvent>) {
        attach_as(registry, host, Origin::Attended)
    }

    fn attach_as(
        registry: &mut Registry,
        host: &str,
        origin: Origin,
    ) -> (SessionId, mpsc::Receiver<PushEvent>) {
        let (tx, rx) = mpsc::channel(8);
        let id = registry.attach(spec(host, origin), tx);
        (id, rx)
    }

    /// 러너 입력 통로를 붙이고 준비 상태로 올린다 — 이 세션을 수신자로 만든다(P4).
    fn connect_target(
        registry: &mut Registry,
        id: &SessionId,
        capacity: usize,
    ) -> mpsc::Receiver<PushEvent> {
        let (tx, rx) = mpsc::channel(capacity);
        let target = DeliveryTarget::new(TargetKind::Monitor, tx);
        let target_id = target.id.clone();
        assert!(registry.set_target(id, target));
        assert!(registry.set_target_ready(id, &target_id, true));
        rx
    }

    // ---- 시험 도우미 끝 ----

    #[test]
    fn attach_does_not_take_any_binding() {
        let mut registry = Registry::new();
        let (id, _rx) = attach(&mut registry, "claude");
        assert!(registry.session(&id).expect("attached").bindings.is_empty());
        assert!(registry.holder(&binding("personal/a@c")).is_none());
    }

    #[test]
    fn newest_become_evicts_and_notifies_the_previous_holder() {
        let mut registry = Registry::new();
        let b = binding("personal/brvclaude@brv");
        let (first, mut first_rx) = attach(&mut registry, "claude");
        let (second, _second_rx) = attach(&mut registry, "codex");
        assert_eq!(
            registry.become_binding(&first, &b).unwrap(),
            Became::Took { evicted: None }
        );
        assert_eq!(
            registry.become_binding(&second, &b).unwrap(),
            Became::Took {
                evicted: Some(first.clone())
            }
        );
        match first_rx.try_recv().expect("eviction notice") {
            PushEvent::Evicted { binding: got, by } => {
                assert_eq!(got, b);
                assert_eq!(by.as_deref(), Some("codex"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
        assert!(!registry.may_act_as(&first, &b));
        assert!(registry.may_act_as(&second, &b));
        assert!(
            !registry
                .session(&first)
                .expect("still attached")
                .bindings
                .contains(&b)
        );
    }

    #[test]
    fn become_is_idempotent_for_the_same_session() {
        let mut registry = Registry::new();
        let b = binding("personal/a@c");
        let (id, _rx) = attach(&mut registry, "claude");
        registry.become_binding(&id, &b).unwrap();
        assert_eq!(registry.become_binding(&id, &b).unwrap(), Became::Already);
    }

    #[test]
    fn one_session_holds_several_bindings() {
        let mut registry = Registry::new();
        let (id, _rx) = attach(&mut registry, "claude");
        for label in ["personal/a@c1", "personal/b@c2", "other/a@c1"] {
            registry.become_binding(&id, &binding(label)).unwrap();
        }
        assert_eq!(registry.session(&id).expect("attached").bindings.len(), 3);
        assert!(registry.may_act_as(&id, &binding("other/a@c1")));
    }

    #[test]
    fn hold_blocks_become_until_the_work_is_finished() {
        let mut registry = Registry::new();
        let b = binding("personal/a@c");
        let (worker, _worker_rx) = attach(&mut registry, "claude");
        let (other, _other_rx) = attach(&mut registry, "codex");
        registry.become_binding(&worker, &b).unwrap();
        registry.hold_acquire(&worker, &b, "01MSG").unwrap();

        let error = registry.become_binding(&other, &b).unwrap_err();
        let held = error.downcast_ref::<Held>().expect("Held error");
        assert_eq!(held.open, vec!["01MSG".to_owned()]);
        assert_eq!(held.holder.as_ref(), Some(&worker));
        assert!(registry.may_act_as(&worker, &b), "holder must not change");

        // 최종 reply/report 확정 = 커밋
        assert!(registry.hold_release(&b, "01MSG"));
        assert!(!registry.is_held(&b));
        assert_eq!(
            registry.become_binding(&other, &b).unwrap(),
            Became::Took {
                evicted: Some(worker)
            }
        );
    }

    #[test]
    fn several_open_works_keep_the_binding_locked() {
        let mut registry = Registry::new();
        let b = binding("personal/a@c");
        let (worker, _rx) = attach(&mut registry, "claude");
        let (other, _other_rx) = attach(&mut registry, "codex");
        registry.become_binding(&worker, &b).unwrap();
        registry.hold_acquire(&worker, &b, "01A").unwrap();
        registry.hold_acquire(&worker, &b, "01B").unwrap();
        assert!(registry.hold_release(&b, "01A"));
        assert!(registry.is_held(&b), "01B is still open");
        assert!(registry.become_binding(&other, &b).is_err());
        assert!(registry.hold_release(&b, "01B"));
        assert!(registry.become_binding(&other, &b).is_ok());
    }

    #[test]
    fn hold_acquire_requires_holding_the_binding() {
        let mut registry = Registry::new();
        let b = binding("personal/a@c");
        let (holder, _rx) = attach(&mut registry, "claude");
        let (stranger, _stranger_rx) = attach(&mut registry, "codex");
        registry.become_binding(&holder, &b).unwrap();
        assert!(registry.hold_acquire(&stranger, &b, "01A").is_err());
    }

    #[test]
    fn releasing_an_unknown_work_is_not_an_error() {
        let mut registry = Registry::new();
        let b = binding("personal/a@c");
        assert!(!registry.hold_release(&b, "01A"));
    }

    #[test]
    fn session_death_releases_holds_and_bindings() {
        let mut registry = Registry::new();
        let b = binding("personal/a@c");
        let (worker, _rx) = attach(&mut registry, "claude");
        let (other, _other_rx) = attach(&mut registry, "codex");
        registry.become_binding(&worker, &b).unwrap();
        registry.hold_acquire(&worker, &b, "01A").unwrap();

        let freed = registry.detach(&worker);
        assert_eq!(freed, vec![b.clone()]);
        assert!(!registry.is_held(&b), "죽은 세션의 잠금은 남지 않는다");
        assert!(registry.holder(&b).is_none());
        assert!(registry.become_binding(&other, &b).is_ok());
    }

    #[test]
    fn detach_of_unknown_session_is_a_no_op() {
        let mut registry = Registry::new();
        assert!(registry.detach(&SessionId::generate()).is_empty());
    }

    #[test]
    fn force_release_unlocks_a_silent_session() {
        let mut registry = Registry::new();
        let b = binding("personal/a@c");
        let (worker, _rx) = attach(&mut registry, "claude");
        let (other, _other_rx) = attach(&mut registry, "codex");
        registry.become_binding(&worker, &b).unwrap();
        registry.hold_acquire(&worker, &b, "01A").unwrap();
        assert_eq!(registry.force_release(&b), vec!["01A".to_owned()]);
        assert!(registry.become_binding(&other, &b).is_ok());
    }

    #[test]
    fn wake_window_is_protected_and_then_adopted_by_the_woken_session() {
        let mut registry = Registry::new();
        let b = binding("personal/a@c");
        let wake = WakeId::generate();
        // 스폰 직후 — 아직 붙은 세션이 없다
        registry.hold_for_wake(&b, &wake, &["01A".to_owned()]);

        // 그 사이 사람이 연 세션은 끼어들 수 없다
        let (human, _human_rx) = attach(&mut registry, "claude");
        assert!(registry.become_binding(&human, &b).is_err());

        // 깨어난 세션은 자기 창이므로 통과하고 잠금을 승계한다
        let (woken, _woken_rx) = attach_as(
            &mut registry,
            "claude",
            Origin::Woken { wake: wake.clone() },
        );
        assert!(registry.become_binding(&woken, &b).is_ok());
        assert_eq!(
            registry.holds(&b).first().map(|h| &h.owner),
            Some(&HoldOwner::Session {
                session: woken.clone()
            })
        );
        // 승계된 뒤에도 여전히 잠겨 있다
        assert!(registry.become_binding(&human, &b).is_err());
        // 그 세션이 죽으면 풀린다
        registry.detach(&woken);
        assert!(registry.become_binding(&human, &b).is_ok());
    }

    #[test]
    fn wake_that_never_attaches_releases_its_window() {
        let mut registry = Registry::new();
        let b = binding("personal/a@c");
        let wake = WakeId::generate();
        registry.hold_for_wake(&b, &wake, &["01A".to_owned()]);
        let (human, _rx) = attach(&mut registry, "claude");
        assert!(registry.become_binding(&human, &b).is_err());
        assert_eq!(registry.release_wake(&wake), vec![b.clone()]);
        assert!(registry.become_binding(&human, &b).is_ok());
    }

    #[test]
    fn another_wake_window_does_not_let_a_foreign_woken_session_in() {
        let mut registry = Registry::new();
        let b = binding("personal/a@c");
        let mine = WakeId::generate();
        registry.hold_for_wake(&b, &mine, &["01A".to_owned()]);
        let (other_woken, _rx) = attach_as(
            &mut registry,
            "codex",
            Origin::Woken {
                wake: WakeId::generate(),
            },
        );
        assert!(registry.become_binding(&other_woken, &b).is_err());
    }

    #[test]
    fn a_holder_without_a_ready_input_path_is_not_a_receiver() {
        // 2026-09-10 P4 정정: 제어 통로만 열린 세션도, 통로를 만들었지만 아직 붙지 않은 세션도
        // 수신자가 아니다 — 모델 턴을 여는 통로가 실제로 붙어야 한다.
        let mut registry = Registry::new();
        let b = binding("personal/a@c");
        let (id, _control) = attach(&mut registry, "claude");
        registry.become_binding(&id, &b).unwrap();
        assert!(registry.holder(&b).is_some(), "정체성은 쥐고 있다");
        assert!(
            registry.receiver_for(&b).is_none(),
            "통로가 없으면 수신자가 아니다"
        );

        let (tx, _target_rx) = mpsc::channel(4);
        let target = DeliveryTarget::new(TargetKind::Monitor, tx);
        let target_id = target.id.clone();
        assert!(registry.set_target(&id, target));
        assert!(
            registry.receiver_for(&b).is_none(),
            "붙기 전에는 수신자가 아니다"
        );
        assert!(registry.set_target_ready(&id, &target_id, true));
        assert!(registry.receiver_for(&b).is_some(), "붙은 뒤에야 수신자다");
    }

    #[test]
    fn delivery_reports_gone_when_the_input_path_closed() {
        let mut registry = Registry::new();
        let b = binding("personal/a@c");
        let (id, _control) = attach(&mut registry, "claude");
        registry.become_binding(&id, &b).unwrap();
        drop(connect_target(&mut registry, &id, 4));
        assert!(
            registry.receiver_for(&b).is_none(),
            "닫힌 통로는 수신자가 아니다"
        );
        let session = registry.session(&id).expect("attached");
        assert_eq!(
            session.deliver(PushEvent::Shutdown).unwrap_err(),
            PushError::Gone
        );
    }

    #[test]
    fn delivery_reports_busy_when_the_input_path_is_full() {
        let mut registry = Registry::new();
        let b = binding("personal/a@c");
        let (id, _control) = attach(&mut registry, "claude");
        registry.become_binding(&id, &b).unwrap();
        let _target_rx = connect_target(&mut registry, &id, 1);
        let session = registry.receiver_for(&b).expect("receiver");
        session.deliver(PushEvent::Shutdown).unwrap();
        assert_eq!(
            session.deliver(PushEvent::Shutdown).unwrap_err(),
            PushError::Busy
        );
    }

    #[test]
    fn delivery_without_an_input_path_is_refused() {
        let mut registry = Registry::new();
        let (id, _control) = attach(&mut registry, "claude");
        let session = registry.session(&id).expect("attached");
        assert_eq!(
            session.deliver(PushEvent::Shutdown).unwrap_err(),
            PushError::NoTarget
        );
    }

    #[test]
    fn a_stale_input_path_notice_does_not_touch_its_replacement() {
        let mut registry = Registry::new();
        let (id, _control) = attach(&mut registry, "claude");
        let (old_tx, _old_rx) = mpsc::channel(1);
        let old = DeliveryTarget::new(TargetKind::Monitor, old_tx);
        let old_id = old.id.clone();
        assert!(registry.set_target(&id, old));
        let _new_rx = connect_target(&mut registry, &id, 1);
        assert!(
            !registry.set_target_ready(&id, &old_id, false),
            "옛 통로의 통지는 무시된다"
        );
        assert!(
            !registry.clear_target(&id, &old_id),
            "옛 통로의 소실 통지는 새 통로를 지우지 않는다"
        );
        assert!(registry.session(&id).expect("attached").can_receive());
    }

    #[test]
    fn release_binding_is_refused_while_held() {
        let mut registry = Registry::new();
        let b = binding("personal/a@c");
        let (id, _rx) = attach(&mut registry, "claude");
        registry.become_binding(&id, &b).unwrap();
        registry.hold_acquire(&id, &b, "01A").unwrap();
        assert!(registry.release_binding(&id, &b).is_err());
        registry.hold_release(&b, "01A");
        assert!(registry.release_binding(&id, &b).is_ok());
        assert!(registry.holder(&b).is_none());
    }

    #[test]
    fn release_binding_requires_being_the_holder() {
        let mut registry = Registry::new();
        let b = binding("personal/a@c");
        let (holder, _rx) = attach(&mut registry, "claude");
        let (stranger, _stranger_rx) = attach(&mut registry, "codex");
        registry.become_binding(&holder, &b).unwrap();
        assert!(registry.release_binding(&stranger, &b).is_err());
    }

    #[test]
    fn become_by_an_unknown_session_is_refused() {
        let mut registry = Registry::new();
        assert!(
            registry
                .become_binding(&SessionId::generate(), &binding("personal/a@c"))
                .is_err()
        );
    }

    #[test]
    fn origin_decides_local_policy_not_the_session_claim() {
        let mut registry = Registry::new();
        let (human, _a) = attach(&mut registry, "claude");
        let (woken, _b) = attach_as(
            &mut registry,
            "claude",
            Origin::Woken {
                wake: WakeId::generate(),
            },
        );
        assert!(
            registry
                .session(&human)
                .expect("attached")
                .origin
                .is_attended()
        );
        assert!(
            !registry
                .session(&woken)
                .expect("attached")
                .origin
                .is_attended()
        );
    }

    #[test]
    fn shutdown_notifies_every_attached_session() {
        let mut registry = Registry::new();
        let (_a, mut a_rx) = attach(&mut registry, "claude");
        let (_b, mut b_rx) = attach(&mut registry, "codex");
        registry.shutdown();
        assert!(matches!(a_rx.try_recv(), Ok(PushEvent::Shutdown)));
        assert!(matches!(b_rx.try_recv(), Ok(PushEvent::Shutdown)));
    }

    #[test]
    fn evicting_a_dead_session_does_not_fail_the_takeover() {
        let mut registry = Registry::new();
        let b = binding("personal/a@c");
        let (first, first_rx) = attach(&mut registry, "claude");
        registry.become_binding(&first, &b).unwrap();
        drop(first_rx); // 전송로가 닫힌 세션
        let (second, _second_rx) = attach(&mut registry, "codex");
        assert_eq!(
            registry.become_binding(&second, &b).unwrap(),
            Became::Took {
                evicted: Some(first)
            }
        );
    }

    #[test]
    fn holds_are_scoped_to_a_binding() {
        let mut registry = Registry::new();
        let (id, _rx) = attach(&mut registry, "claude");
        let (b1, b2) = (binding("personal/a@c1"), binding("personal/a@c2"));
        registry.become_binding(&id, &b1).unwrap();
        registry.become_binding(&id, &b2).unwrap();
        registry.hold_acquire(&id, &b1, "01A").unwrap();
        assert!(registry.is_held(&b1));
        assert!(!registry.is_held(&b2), "다른 바인딩은 잠기지 않는다");
    }
}
