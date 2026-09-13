// Copyright 2026 SEIZIA (Jaeyoung Ko)
// SPDX-License-Identifier: Apache-2.0

//! 로컬 평면의 MCP 프런트엔드 — 루프백 HTTP (RECEIVER_DESIGN.md P3).
//!
//! **왜 HTTP인가**: stdio MCP는 러너가 서버를 자식 프로세스로 띄우는 규약이라, 리시버가 미리
//! 띄워 둔 것에 러너를 붙일 방법이 없다. 러너가 **우리에게 오게** 하려면 전송이 HTTP여야
//! 한다. 그래야 세션마다 리시버 로직 사본이 새로 뜨는 일도, 갱신 뒤 옛 사본이 살아남는 일도
//! 없다(P8). HTTP를 지원하지 않는 러너에는 얇은 stdio 브리지만 남는다.
//!
//! MCP Streamable HTTP 규약을 따른다: 단일 경로 `/mcp`,
//! - `POST` — JSON-RPC 요청. 응답이 있으면 `application/json`, 알림·응답뿐이면 `202`.
//! - `GET` — `text/event-stream`. 리시버→세션 **제어 통로**(밀려남·종료 통지)다. 이 스트림이
//!   열렸다고 수신자가 되지는 않는다 — 일반 MCP 호스트는 알림으로 모델 턴을 열지 않기
//!   때문이다(P4, 2026-09-10 정정). 수신자 판정은 러너 입력 통로가 붙었는가로 평면이 한다.
//!   스트림이 닫히면 세션이 끝난 것으로 본다.
//! - `DELETE` — 세션 종료.
//!
//! 운영자 표면은 세션 없이 같은 자격으로 부른다: `/status`(조회), `/publish`(`brv send`), `/listen`(`brv listen`,
//! 리시버 관찰 — 2026-09-11 확정, 17단계).
//!
//! 자격은 [`super::auth`]: 루프백 바인딩, `Authorization: Bearer`, `Origin` 검증.
//!
//! 전송 계층은 여기까지만 안다. 도구·라우팅은 [`SessionHandler`] 뒤에 있고, 로봇 제어기는
//! 같은 등록부에 다른 프런트엔드로 붙는다(REBUILD_PLAN §1.3).

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use bytes::Bytes;
use http_body_util::{BodyExt as _, Full, StreamBody, combinators::BoxBody};
use hyper::body::Frame;
use hyper::header::{
    ACCEPT, AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, HeaderName, HeaderValue, ORIGIN,
};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, mpsc};

use super::auth::{Token, bearer, is_loopback, origin_allowed};
use super::registry::{AttachSpec, PushEvent, SessionId, Sink};

/// MCP 세션 헤더 (스펙 고정 이름).
const SESSION_HEADER: &str = "mcp-session-id";
/// 브리지가 모든 요청에 싣는 자기 버전 (2026-09-12, 10단계·P8) — 기동 때 한 번 대조하던 것을 요청마다로 넓혔다.
/// 갱신 전부터 러너 안에 떠 있던 브리지가 새 리시버와 옛 규약으로 대화하지 않게, 다르면 이유와 함께 거부한다.
/// 헤더가 없는 직접 HTTP 클라이언트(러너의 HTTP MCP 등록)는 그대로 받는다.
pub const BRIDGE_VERSION_HEADER: &str = "x-brv-bridge-version";

/// 다른 버전의 브리지면 거부 사유.
fn stale_bridge(version: Option<&str>) -> Option<String> {
    let version = version?;
    (version != env!("CARGO_PKG_VERSION")).then(|| {
        format!(
            "this brv bridge is {version} but the running receiver is {} — restart this session's Brevduva MCP (the app's MCP restart action, or relaunch the CLI session) so the runner starts the current bridge",
            env!("CARGO_PKG_VERSION")
        )
    })
}
/// 세션이 소화하지 못한 밀어넣기가 이만큼 쌓이면 그 다음은 `Busy` — P6에 따라 큐에 남는다.
const SINK_CAPACITY: usize = 32;
/// SSE 유휴 시 주석 프레임 간격 — 중간의 프록시·방화벽이 조용한 연결을 끊지 않게.
const SSE_KEEPALIVE: Duration = Duration::from_secs(20);

type Body = BoxBody<Bytes, Infallible>;
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 전송 계층이 도구·라우팅 계층에 요구하는 것. 구현은 [`super::plane`]에 있고, 테스트는
/// 가벼운 대역을 끼운다.
pub trait SessionHandler: Send + Sync + 'static {
    /// 세션 등록. 이 시점에는 어떤 바인딩도 쥐지 않는다 — 정체성은 `become`이 정한다.
    fn attach(&self, spec: AttachSpec, sink: Sink) -> SessionId;
    /// 전송로가 닫혔다 — 쥔 바인딩과 잠금을 놓는다.
    fn detach(&self, id: &SessionId);
    /// JSON-RPC 요청 처리. 알림이면 `None`.
    fn dispatch<'a>(&'a self, id: &'a SessionId, request: Value) -> BoxFuture<'a, Option<Value>>;
    /// 운영자 조회 — 세션이 아니라 사람이 부른다. 서버에 새 JOIN을 만들지 않는다(P2).
    fn status(&self) -> BoxFuture<'_, Value>;
    /// 운영자 발행 — `brv send`. 리시버가 쥔 접속으로 보낸다(P2, 8단계).
    fn publish(&self, body: Value) -> BoxFuture<'_, Value>;
    /// 운영자 관찰 — `brv listen`. 리시버의 판단을 흘려 보일 뿐 아무것도 가져가지 않는다(17단계).
    fn tap(&self) -> tokio::sync::broadcast::Receiver<Value>;
}

/// 로컬 엔드포인트 상태.
pub struct LocalHttp {
    token: Token,
    handler: Arc<dyn SessionHandler>,
    /// 아직 SSE로 인계되지 않은 밀어넣기 수신단. `initialize`에서 만들고 `GET`이 가져간다.
    pending: Mutex<HashMap<SessionId, mpsc::Receiver<PushEvent>>>,
}

impl LocalHttp {
    pub fn new(token: Token, handler: Arc<dyn SessionHandler>) -> Arc<Self> {
        Arc::new(Self {
            token,
            handler,
            pending: Mutex::new(HashMap::new()),
        })
    }
}

/// 루프백에만 연다. 다른 주소로는 바인딩하지 않는다(U5).
pub async fn bind(port: u16) -> anyhow::Result<TcpListener> {
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port)))
        .await
        .with_context(|| format!("cannot open the local receiver endpoint on port {port}"))?;
    let addr = listener.local_addr()?;
    anyhow::ensure!(is_loopback(&addr), "local endpoint bound off loopback");
    Ok(listener)
}

/// 접속을 받는다. 종료 신호가 오면 새 접속을 그만 받는다.
pub async fn serve(
    listener: TcpListener,
    state: Arc<LocalHttp>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            _ = shutdown.changed() => return Ok(()),
        };
        let (stream, peer) = match accepted {
            Ok(pair) => pair,
            Err(error) => {
                tracing::warn!(%error, "local endpoint accept failed");
                continue;
            }
        };
        // 루프백 리스너라 원격 피어가 올 수 없지만, 사고로라도 새면 여기서 끊는다.
        if !is_loopback(&peer) {
            tracing::warn!(%peer, "refused a non-loopback peer on the local endpoint");
            continue;
        }
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let io = hyper_util::rt::TokioIo::new(stream);
            let service = service_fn(move |request| {
                let state = Arc::clone(&state);
                async move { Ok::<_, Infallible>(route(state, request).await) }
            });
            if let Err(error) = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, service)
                .await
            {
                tracing::debug!(%error, "local endpoint connection ended");
            }
        });
    }
}

fn text(status: StatusCode, message: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from(message.to_owned())).boxed())
        .expect("static response")
}

fn json_response(status: StatusCode, value: &Value, session: Option<&SessionId>) -> Response<Body> {
    let mut builder = Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json");
    if let Some(id) = session {
        builder = builder.header(
            HeaderName::from_static(SESSION_HEADER),
            HeaderValue::from_str(id.as_str()).expect("session id is ascii"),
        );
    }
    builder
        .body(Full::new(Bytes::from(value.to_string())).boxed())
        .expect("json response")
}

fn header(request: &Request<hyper::body::Incoming>, name: HeaderName) -> Option<&str> {
    request.headers().get(name)?.to_str().ok()
}

fn session_header(request: &Request<hyper::body::Incoming>) -> Option<SessionId> {
    header(request, HeaderName::from_static(SESSION_HEADER)).map(|raw| SessionId::parse(raw.trim()))
}

async fn route(state: Arc<LocalHttp>, request: Request<hyper::body::Incoming>) -> Response<Body> {
    // 1) 브라우저발 차단 (DNS 리바인딩) — 자격 검사보다 먼저.
    if !origin_allowed(header(&request, ORIGIN)) {
        return text(StatusCode::FORBIDDEN, "origin not allowed");
    }
    // 2) 자격.
    let presented = bearer(header(&request, AUTHORIZATION));
    if !presented.is_some_and(|token| state.token.matches(token)) {
        return Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header("www-authenticate", "Bearer")
            .header(CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(
                Full::new(Bytes::from_static(
                    b"present the local receiver token from endpoint.json",
                ))
                .boxed(),
            )
            .expect("static response");
    }
    // 운영자 조회 — 세션 없이 부른다. 이 경로는 서버 접속을 새로 만들지 않는다(P2).
    if request.uri().path() == "/status" {
        return if request.method() == Method::GET {
            json_response(StatusCode::OK, &state.handler.status().await, None)
        } else {
            text(StatusCode::METHOD_NOT_ALLOWED, "use GET")
        };
    }
    // 운영자 발행 — CLI가 서버에 직접 붙지 않게 리시버의 접속으로 보낸다(P2, 8단계).
    if request.uri().path() == "/publish" {
        if request.method() != Method::POST {
            return text(StatusCode::METHOD_NOT_ALLOWED, "use POST");
        }
        let body = match request.into_body().collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => return text(StatusCode::BAD_REQUEST, "could not read the request body"),
        };
        return match serde_json::from_slice::<Value>(&body) {
            Ok(body) => json_response(StatusCode::OK, &state.handler.publish(body).await, None),
            Err(_) => text(StatusCode::BAD_REQUEST, "the publish body must be JSON"),
        };
    }
    // 운영자 관찰 — `brv listen`(2026-09-11 확정, 17단계). 리시버가 받은 메시지와 행선지를 줄 단위 JSON으로
    // 흘린다. 서버에 붙지 않고 아무것도 가져가지 않는다(P2) — 옛 `listen`은 JOIN해 받은 것을 소비했다.
    if request.uri().path() == "/listen" {
        return if request.method() == Method::GET {
            listen_stream(state.handler.tap())
        } else {
            text(StatusCode::METHOD_NOT_ALLOWED, "use GET")
        };
    }
    if request.uri().path() != "/mcp" {
        return text(
            StatusCode::NOT_FOUND,
            "the local receiver serves /mcp, /status, /publish and /listen",
        );
    }
    match *request.method() {
        Method::POST => post(state, request).await,
        Method::GET => get(state, request).await,
        Method::DELETE => delete(state, request).await,
        _ => text(StatusCode::METHOD_NOT_ALLOWED, "use POST, GET or DELETE"),
    }
}

/// JSON-RPC 요청. 세션이 없으면 `initialize`만 받는다 — 그 자리에서 세션을 만든다.
async fn post(state: Arc<LocalHttp>, request: Request<hyper::body::Incoming>) -> Response<Body> {
    let existing = session_header(&request);
    let bridge =
        header(&request, HeaderName::from_static(BRIDGE_VERSION_HEADER)).map(str::to_owned);
    let body = match request.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return text(StatusCode::BAD_REQUEST, "could not read the request body"),
    };
    let Ok(message) = serde_json::from_slice::<Value>(&body) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            &rpc_error(Value::Null, -32700, "parse error"),
            None,
        );
    };
    let method = message["method"].as_str().unwrap_or_default().to_owned();
    if let Some(reason) = stale_bridge(bridge.as_deref()) {
        return json_response(
            StatusCode::CONFLICT,
            &rpc_error(message["id"].clone(), -32001, &reason),
            None,
        );
    }

    let (session, fresh) = match existing {
        Some(id) => (id, false),
        None => {
            if method != "initialize" {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    &rpc_error(
                        message["id"].clone(),
                        -32600,
                        "no local session — send initialize first (the receiver assigns Mcp-Session-Id)",
                    ),
                    None,
                );
            }
            let (sink, receiver) = mpsc::channel(SINK_CAPACITY);
            let id = state.handler.attach(attach_spec(&message), sink);
            state.pending.lock().await.insert(id.clone(), receiver);
            (id, true)
        }
    };

    match state.handler.dispatch(&session, message).await {
        // 알림·응답에는 본문이 없다.
        None => Response::builder()
            .status(StatusCode::ACCEPTED)
            .body(Full::new(Bytes::new()).boxed())
            .expect("accepted"),
        Some(response) => json_response(
            StatusCode::OK,
            &response,
            // 세션 id는 만든 그 응답에서만 알려주면 된다(스펙) — 이후엔 클라이언트가 붙인다.
            fresh.then_some(&session),
        ),
    }
}

/// `initialize`의 `clientInfo`에서 세션 정체를 읽는다. 추측하지 않는다 — 없으면 None이다.
fn attach_spec(message: &Value) -> AttachSpec {
    let info = &message["params"]["clientInfo"];
    let host = info["name"].as_str().map(str::to_owned);
    let description = info["version"]
        .as_str()
        .map(|v| format!("{} {v}", host.as_deref().unwrap_or("client")));
    AttachSpec {
        host,
        // 깨운 세션인지는 세션의 주장이 아니라 리시버의 기억으로 정한다 — 도구·라우팅 계층이
        // `BREVDUVA_WAKE` 대조로 승격시킨다(REBUILD_PLAN §1.5).
        origin: super::registry::Origin::Attended,
        capabilities: super::registry::SessionCapabilities::default(),
        description,
    }
}

/// 밀어넣기 통로. 이 스트림이 열려 있는 동안만 그 세션은 "받을 수 있다"(P4).
async fn get(state: Arc<LocalHttp>, request: Request<hyper::body::Incoming>) -> Response<Body> {
    if let Some(reason) = stale_bridge(header(
        &request,
        HeaderName::from_static(BRIDGE_VERSION_HEADER),
    )) {
        return text(StatusCode::CONFLICT, &reason);
    }
    let accept = header(&request, ACCEPT).unwrap_or_default().to_owned();
    if !accept.contains("text/event-stream") && !accept.contains("*/*") {
        return text(
            StatusCode::NOT_ACCEPTABLE,
            "the delivery stream is text/event-stream",
        );
    }
    let Some(session) = session_header(&request) else {
        return text(
            StatusCode::BAD_REQUEST,
            "Mcp-Session-Id is required for the delivery stream",
        );
    };
    let Some(receiver) = state.pending.lock().await.remove(&session) else {
        return text(
            StatusCode::CONFLICT,
            "this session has no delivery stream to open (unknown session, or one is already open)",
        );
    };
    // 제어 통로가 열렸다 — 밀려남·종료 통지가 이리로 온다. 이것만으로 수신자가 되지는 않는다:
    // 모델 턴을 여는 러너 입력 통로가 따로 붙어야 한다(P4, 2026-09-10 정정).
    let guard = StreamGuard {
        state: Arc::clone(&state),
        session,
        receiver: Some(receiver),
    };
    let stream = futures_util::stream::unfold(guard, |mut guard| async move {
        let receiver = guard.receiver.as_mut().expect("receiver present");
        match tokio::time::timeout(SSE_KEEPALIVE, receiver.recv()).await {
            // 등록부가 세션을 지웠다(밀려남·종료) — 스트림을 닫는다.
            Ok(None) => None,
            Ok(Some(event)) => {
                let frame = Frame::data(Bytes::from(sse_frame(&event)));
                if matches!(event, PushEvent::Shutdown) {
                    // 마지막 프레임을 보내고 닫는다.
                    guard.receiver.as_mut().expect("receiver present").close();
                }
                Some((Ok::<_, Infallible>(frame), guard))
            }
            // 유휴 — 주석 프레임으로 연결을 살려 둔다.
            Err(_) => Some((
                Ok(Frame::data(Bytes::from_static(b": keep-alive\n\n"))),
                guard,
            )),
        }
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .header(CACHE_CONTROL, "no-store")
        .body(StreamBody::new(stream).boxed())
        .expect("sse response")
}

async fn delete(state: Arc<LocalHttp>, request: Request<hyper::body::Incoming>) -> Response<Body> {
    let Some(session) = session_header(&request) else {
        return text(StatusCode::BAD_REQUEST, "Mcp-Session-Id is required");
    };
    state.pending.lock().await.remove(&session);
    state.handler.detach(&session);
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Full::new(Bytes::new()).boxed())
        .expect("no content")
}

/// 관찰 흐름 → 줄 단위 JSON (17단계). 유휴에는 빈 줄로 연결을 살피고, 밀린 사건은 건수만 알린다.
fn listen_stream(receiver: tokio::sync::broadcast::Receiver<Value>) -> Response<Body> {
    let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        let line = match tokio::time::timeout(SSE_KEEPALIVE, receiver.recv()).await {
            Ok(Ok(event)) => format!("{event}\n"),
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(missed))) => {
                format!("{}\n", json!({"event": "lagged", "missed": missed}))
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => return None,
            Err(_) => "\n".to_owned(),
        };
        Some((
            Ok::<_, Infallible>(Frame::data(Bytes::from(line))),
            receiver,
        ))
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/x-ndjson")
        .header(CACHE_CONTROL, "no-store")
        .body(StreamBody::new(stream).boxed())
        .expect("listen response")
}

/// SSE 스트림이 끝나면 — 클라이언트가 끊었든 리시버가 닫았든 — 세션을 놓는다.
struct StreamGuard {
    state: Arc<LocalHttp>,
    session: SessionId,
    receiver: Option<mpsc::Receiver<PushEvent>>,
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        // 제어 통로가 끊겼다 = 세션이 끝났다. 쥐고 있던 바인딩·잠금은 등록부가 놓고(U2: 세션
        // 사망 = 잠금 해제), 넘겨 두고 수락받지 못한 전달은 평면이 결과 불명으로 남긴다.
        self.state.handler.detach(&self.session);
    }
}

/// 밀어넣기 사건 → JSON-RPC 알림 → SSE 프레임.
fn sse_frame(event: &PushEvent) -> String {
    format!("event: message\ndata: {}\n\n", notification(event))
}

/// 로컬 평면의 알림 어휘. 러너별 입력 통로(Claude Monitor·Codex queue)는 이 위에 얹힌다.
pub fn notification(event: &PushEvent) -> Value {
    match event {
        PushEvent::Message {
            binding,
            envelope,
            receipt,
        } => json!({
            "jsonrpc": "2.0",
            "method": "notifications/brevduva/message",
            "params": {
                "binding": binding.as_str(),
                "receipt": receipt,
                "envelope": envelope,
            },
        }),
        PushEvent::Evicted { binding, by } => json!({
            "jsonrpc": "2.0",
            "method": "notifications/brevduva/evicted",
            "params": {
                "binding": binding.as_str(),
                "by": by,
                "detail": "another session took this binding; call become again to take it back",
            },
        }),
        // Claude Code Channels 규격의 알림 이름 그대로 (7c)
        PushEvent::ChannelEvent { content, meta } => json!({
            "jsonrpc": "2.0",
            "method": "notifications/claude/channel",
            "params": {"content": content, "meta": meta},
        }),
        PushEvent::Shutdown => json!({
            "jsonrpc": "2.0",
            "method": "notifications/brevduva/shutdown",
            "params": {"detail": "the receiver is going down"},
        }),
    }
}

fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_plane::registry::{BindingKey, Registry};
    use std::sync::Mutex as StdMutex;

    /// 등록부만 붙인 최소 대역 — 도구 계층(4단계) 없이 전송 계층을 검증한다.
    struct TestHandler {
        registry: StdMutex<Registry>,
        /// 마지막으로 붙은 세션 — 테스트가 밀어넣기를 걸어 본다.
        last: StdMutex<Option<SessionId>>,
        tap: tokio::sync::broadcast::Sender<Value>,
    }

    impl TestHandler {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                registry: StdMutex::new(Registry::new()),
                last: StdMutex::new(None),
                tap: tokio::sync::broadcast::channel(16).0,
            })
        }
    }

    impl SessionHandler for TestHandler {
        fn tap(&self) -> tokio::sync::broadcast::Receiver<Value> {
            self.tap.subscribe()
        }

        fn publish(&self, body: Value) -> BoxFuture<'_, Value> {
            Box::pin(async move { json!({"status": "sent", "echo": body}) })
        }

        fn attach(&self, spec: AttachSpec, sink: Sink) -> SessionId {
            let id = self.registry.lock().expect("registry").attach(spec, sink);
            *self.last.lock().expect("last") = Some(id.clone());
            id
        }

        fn detach(&self, id: &SessionId) {
            self.registry.lock().expect("registry").detach(id);
        }

        fn status(&self) -> BoxFuture<'_, Value> {
            Box::pin(async move { json!({"sessions": [], "bindings": []}) })
        }

        fn dispatch<'a>(
            &'a self,
            id: &'a SessionId,
            request: Value,
        ) -> BoxFuture<'a, Option<Value>> {
            Box::pin(async move {
                let rpc_id = request.get("id").cloned()?;
                let method = request["method"].as_str().unwrap_or_default();
                let result = match method {
                    "initialize" => {
                        json!({"protocolVersion": "2025-06-18", "session": id.as_str()})
                    }
                    "ping" => json!({}),
                    _ => return Some(rpc_error(rpc_id, -32601, "method not found")),
                };
                Some(json!({"jsonrpc": "2.0", "id": rpc_id, "result": result}))
            })
        }
    }

    struct Harness {
        base: String,
        token: Token,
        handler: Arc<TestHandler>,
        shutdown: tokio::sync::watch::Sender<bool>,
    }

    async fn start() -> Harness {
        let token = Token::generate().expect("token");
        let handler = TestHandler::new();
        let state = LocalHttp::new(
            token.clone(),
            Arc::clone(&handler) as Arc<dyn SessionHandler>,
        );
        let listener = bind(0).await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (shutdown, rx) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            let _ = serve(listener, state, rx).await;
        });
        Harness {
            base: format!("http://{addr}/mcp"),
            token,
            handler,
            shutdown,
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            let _ = self.shutdown.send(true);
        }
    }

    #[tokio::test]
    async fn the_listen_stream_shows_what_the_receiver_does_and_needs_the_token() {
        // 17단계: `brv listen`은 리시버의 관찰 흐름을 읽는다 — 같은 자격이 필요하다
        let h = start().await;
        let listen = h.base.replace("/mcp", "/listen");
        let refused = client().get(&listen).send().await.expect("unauthenticated");
        assert_eq!(refused.status().as_u16(), 401);
        let mut response = client()
            .get(&listen)
            .bearer_auth(h.token.expose())
            .send()
            .await
            .expect("listen");
        assert_eq!(response.status().as_u16(), 200);
        for _ in 0..100 {
            if h.handler.tap.receiver_count() > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        h.handler
            .tap
            .send(json!({"event": "received", "route": "unattended"}))
            .expect("the stream subscribed");
        let chunk = tokio::time::timeout(Duration::from_secs(5), response.chunk())
            .await
            .expect("event in time")
            .expect("read")
            .expect("a chunk");
        let line: Value = serde_json::from_slice(chunk.trim_ascii()).expect("json line");
        assert_eq!(line["route"], "unattended");
    }

    #[tokio::test]
    async fn a_bridge_from_another_version_is_refused_with_the_reason() {
        // 2026-09-12(10단계, P8): 갱신 전부터 떠 있던 브리지는 요청마다 거부되고 이유를 받는다 — 같은 버전과
        // 헤더 없는 직접 HTTP 클라이언트는 그대로 받는다
        let h = start().await;
        let initialize =
            |id: i64| json!({"jsonrpc": "2.0", "id": id, "method": "initialize", "params": {}});
        let refused = client()
            .post(&h.base)
            .bearer_auth(h.token.expose())
            .header(BRIDGE_VERSION_HEADER, "0.0.1")
            .json(&initialize(7))
            .send()
            .await
            .expect("post");
        assert_eq!(refused.status().as_u16(), 409);
        let body: Value = refused.json().await.expect("json");
        assert_eq!(body["id"], 7);
        assert!(
            body["error"]["message"]
                .as_str()
                .expect("message")
                .contains("restart this session's Brevduva MCP")
        );
        let same = client()
            .post(&h.base)
            .bearer_auth(h.token.expose())
            .header(BRIDGE_VERSION_HEADER, env!("CARGO_PKG_VERSION"))
            .json(&initialize(8))
            .send()
            .await
            .expect("post");
        assert_eq!(same.status().as_u16(), 200);
        let direct = client()
            .post(&h.base)
            .bearer_auth(h.token.expose())
            .json(&initialize(9))
            .send()
            .await
            .expect("post");
        assert_eq!(direct.status().as_u16(), 200);
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client")
    }

    async fn initialize(h: &Harness) -> (reqwest::Response, String) {
        let response = client()
            .post(&h.base)
            .bearer_auth(h.token.expose())
            .json(&json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"clientInfo": {"name": "claude", "version": "2.1.263"}}
            }))
            .send()
            .await
            .expect("initialize");
        let session = response
            .headers()
            .get(SESSION_HEADER)
            .map(|v| v.to_str().expect("ascii").to_owned())
            .unwrap_or_default();
        (response, session)
    }

    #[tokio::test]
    async fn initialize_creates_a_session_and_returns_its_id() {
        let h = start().await;
        let (response, session) = initialize(&h).await;
        assert_eq!(response.status(), 200);
        assert!(!session.is_empty(), "리시버가 세션 id를 발급한다");
        let body: Value = response.json().await.expect("json");
        assert_eq!(body["result"]["session"], session);
        // 붙었지만 아직 아무 바인딩도 쥐지 않았고, 받을 수도 없다 (스트림 미개방)
        let registry = h.handler.registry.lock().expect("registry");
        let id = SessionId::parse(&session);
        let attached = registry.session(&id).expect("attached");
        assert_eq!(attached.host.as_deref(), Some("claude"));
        assert!(attached.bindings.is_empty());
        assert!(!attached.can_receive(), "스트림 전에는 받을 수 없다");
    }

    #[tokio::test]
    async fn requests_without_a_token_are_refused() {
        let h = start().await;
        let response = client()
            .post(&h.base)
            .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))
            .send()
            .await
            .expect("send");
        assert_eq!(response.status(), 401);
        assert!(response.headers().contains_key("www-authenticate"));
    }

    #[tokio::test]
    async fn a_wrong_token_is_refused() {
        let h = start().await;
        let response = client()
            .post(&h.base)
            .bearer_auth("0".repeat(64))
            .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))
            .send()
            .await
            .expect("send");
        assert_eq!(response.status(), 401);
    }

    #[tokio::test]
    async fn a_browser_origin_is_refused_even_with_the_token() {
        let h = start().await;
        let response = client()
            .post(&h.base)
            .bearer_auth(h.token.expose())
            .header(ORIGIN, "http://evil.example")
            .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))
            .send()
            .await
            .expect("send");
        assert_eq!(response.status(), 403, "DNS 리바인딩 차단");
    }

    #[tokio::test]
    async fn a_loopback_origin_is_allowed() {
        let h = start().await;
        let (_, session) = initialize(&h).await;
        let response = client()
            .post(&h.base)
            .bearer_auth(h.token.expose())
            .header(ORIGIN, "http://127.0.0.1:1234")
            .header(SESSION_HEADER, &session)
            .json(&json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}))
            .send()
            .await
            .expect("send");
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn the_first_message_must_be_initialize() {
        let h = start().await;
        let response = client()
            .post(&h.base)
            .bearer_auth(h.token.expose())
            .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
            .send()
            .await
            .expect("send");
        assert_eq!(response.status(), 400);
        let body: Value = response.json().await.expect("json");
        assert!(
            body["error"]["message"]
                .as_str()
                .expect("message")
                .contains("initialize")
        );
    }

    #[tokio::test]
    async fn notifications_get_no_body() {
        let h = start().await;
        let (_, session) = initialize(&h).await;
        let response = client()
            .post(&h.base)
            .bearer_auth(h.token.expose())
            .header(SESSION_HEADER, &session)
            .json(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .send()
            .await
            .expect("send");
        assert_eq!(response.status(), 202);
        assert!(response.bytes().await.expect("body").is_empty());
    }

    #[tokio::test]
    async fn unparsable_bodies_are_a_jsonrpc_parse_error() {
        let h = start().await;
        let response = client()
            .post(&h.base)
            .bearer_auth(h.token.expose())
            .header(CONTENT_TYPE, "application/json")
            .body("{ not json")
            .send()
            .await
            .expect("send");
        assert_eq!(response.status(), 400);
        let body: Value = response.json().await.expect("json");
        assert_eq!(body["error"]["code"], -32700);
    }

    #[tokio::test]
    async fn other_paths_and_methods_are_refused() {
        let h = start().await;
        let base = h.base.trim_end_matches("/mcp").to_owned();
        let response = client()
            .get(format!("{base}/somewhere"))
            .bearer_auth(h.token.expose())
            .send()
            .await
            .expect("send");
        assert_eq!(response.status(), 404);
        let response = client()
            .put(&h.base)
            .bearer_auth(h.token.expose())
            .send()
            .await
            .expect("send");
        assert_eq!(response.status(), 405);
    }

    #[tokio::test]
    async fn the_control_stream_carries_notices_but_does_not_make_a_receiver() {
        let h = start().await;
        let (_, session) = initialize(&h).await;
        let id = SessionId::parse(&session);
        let mut stream = client()
            .get(&h.base)
            .bearer_auth(h.token.expose())
            .header(ACCEPT, "text/event-stream")
            .header(SESSION_HEADER, &session)
            .send()
            .await
            .expect("stream");
        assert_eq!(stream.status(), 200);
        assert_eq!(
            stream
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("text/event-stream")
        );

        // 스트림이 열려도 수신자가 아니다 — 모델 턴을 여는 러너 입력 통로가 따로 붙어야 한다
        // (P4, 2026-09-10 정정). 제어 통지는 이 스트림으로 간다.
        let binding = BindingKey::parse("personal/a@c");
        let pushed = {
            let mut registry = h.handler.registry.lock().expect("registry");
            registry.become_binding(&id, &binding).expect("become");
            assert!(
                registry.receiver_for(&binding).is_none(),
                "제어 통로만 열린 세션은 수신자가 아니다"
            );
            let holder = registry.holder(&binding).expect("holder");
            holder.push(PushEvent::Evicted {
                binding: binding.clone(),
                by: Some("codex".into()),
            })
        };
        assert!(pushed.is_ok(), "제어 통지는 스트림으로 간다");

        let chunk = tokio::time::timeout(Duration::from_secs(5), stream.chunk())
            .await
            .expect("no keep-alive stall")
            .expect("chunk")
            .expect("some bytes");
        let text = String::from_utf8_lossy(&chunk);
        assert!(
            text.starts_with("event: message\ndata: "),
            "SSE 프레임 형식"
        );
        let payload: Value =
            serde_json::from_str(text.trim_start_matches("event: message\ndata: ").trim())
                .expect("json");
        assert_eq!(payload["method"], "notifications/brevduva/evicted");
        assert_eq!(payload["params"]["by"], "codex");
    }

    #[test]
    fn channel_events_render_as_claude_channel_notifications() {
        let mut meta = serde_json::Map::new();
        meta.insert("receipt_token".into(), json!("t"));
        let rendered = notification(&PushEvent::ChannelEvent {
            content: "check".into(),
            meta,
        });
        assert_eq!(rendered["method"], "notifications/claude/channel");
        assert_eq!(rendered["params"]["content"], "check");
        assert_eq!(rendered["params"]["meta"]["receipt_token"], "t");
        assert!(rendered.get("id").is_none(), "알림에는 id가 없다");
    }

    #[tokio::test]
    async fn the_operator_publish_path_requires_the_token_and_post() {
        let h = start().await;
        let base = h.base.trim_end_matches("/mcp").to_owned();
        let body = json!({"binding": "personal/a@c", "to": "peer", "payload": "hi"});
        let refused = client()
            .post(format!("{base}/publish"))
            .json(&body)
            .send()
            .await
            .expect("send");
        assert_eq!(
            refused.status(),
            401,
            "토큰 없이는 이 머신의 정체성으로 보낼 수 없다"
        );
        let wrong_method = client()
            .get(format!("{base}/publish"))
            .bearer_auth(h.token.expose())
            .send()
            .await
            .expect("send");
        assert_eq!(wrong_method.status(), 405);
        let sent: Value = client()
            .post(format!("{base}/publish"))
            .bearer_auth(h.token.expose())
            .json(&body)
            .send()
            .await
            .expect("send")
            .json()
            .await
            .expect("json");
        assert_eq!(sent["status"], "sent");
        assert_eq!(sent["echo"], body);
    }

    #[tokio::test]
    async fn only_one_delivery_stream_per_session() {
        let h = start().await;
        let (_, session) = initialize(&h).await;
        let _first = client()
            .get(&h.base)
            .bearer_auth(h.token.expose())
            .header(ACCEPT, "text/event-stream")
            .header(SESSION_HEADER, &session)
            .send()
            .await
            .expect("stream");
        let second = client()
            .get(&h.base)
            .bearer_auth(h.token.expose())
            .header(ACCEPT, "text/event-stream")
            .header(SESSION_HEADER, &session)
            .send()
            .await
            .expect("send");
        assert_eq!(second.status(), 409);
    }

    #[tokio::test]
    async fn a_stream_for_an_unknown_session_is_refused() {
        let h = start().await;
        let response = client()
            .get(&h.base)
            .bearer_auth(h.token.expose())
            .header(ACCEPT, "text/event-stream")
            .header(SESSION_HEADER, SessionId::generate().as_str())
            .send()
            .await
            .expect("send");
        assert_eq!(response.status(), 409);
    }

    #[tokio::test]
    async fn delete_releases_the_session() {
        let h = start().await;
        let (_, session) = initialize(&h).await;
        let id = SessionId::parse(&session);
        let response = client()
            .delete(&h.base)
            .bearer_auth(h.token.expose())
            .header(SESSION_HEADER, &session)
            .send()
            .await
            .expect("send");
        assert_eq!(response.status(), 204);
        assert!(
            h.handler
                .registry
                .lock()
                .expect("registry")
                .session(&id)
                .is_none()
        );
    }

    #[tokio::test]
    async fn dropping_the_stream_detaches_the_session_and_frees_its_holds() {
        let h = start().await;
        let (_, session) = initialize(&h).await;
        let id = SessionId::parse(&session);
        let binding = BindingKey::parse("personal/a@c");
        let stream = client()
            .get(&h.base)
            .bearer_auth(h.token.expose())
            .header(ACCEPT, "text/event-stream")
            .header(SESSION_HEADER, &session)
            .send()
            .await
            .expect("stream");
        {
            let mut registry = h.handler.registry.lock().expect("registry");
            registry.become_binding(&id, &binding).expect("become");
            registry.hold_acquire(&id, &binding, "01A").expect("hold");
            assert!(registry.is_held(&binding));
        }
        drop(stream);
        // 서버 쪽 스트림 태스크가 종료를 알아채는 데 한 틱이 걸린다
        for _ in 0..50 {
            if h.handler
                .registry
                .lock()
                .expect("registry")
                .session(&id)
                .is_none()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let registry = h.handler.registry.lock().expect("registry");
        assert!(
            registry.session(&id).is_none(),
            "전송로가 닫히면 세션도 없다"
        );
        assert!(!registry.is_held(&binding), "세션 사망 = 잠금 해제 (U2)");
        assert!(registry.holder(&binding).is_none());
    }

    #[tokio::test]
    async fn a_stream_request_must_accept_event_stream() {
        let h = start().await;
        let (_, session) = initialize(&h).await;
        let response = client()
            .get(&h.base)
            .bearer_auth(h.token.expose())
            .header(ACCEPT, "application/json")
            .header(SESSION_HEADER, &session)
            .send()
            .await
            .expect("send");
        assert_eq!(response.status(), 406);
    }
}
