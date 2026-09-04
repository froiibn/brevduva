# Brevduva Protocol Design v0.3 (draft)

[한국어](PROTOCOL.md) · **English**

> This is an **informative translation**. The Korean [PROTOCOL.md](PROTOCOL.md) and the JSON Schemas in [schemas/](schemas/) are normative; where this translation disagrees with them, they win.

> This document is the single source of truth for the Brevduva protocol. Server internals (storage/delivery mapping, HA) live in a private server document — this document contains only the public spec, and a client needs to know nothing beyond what is defined here.

- Status: draft — second design round complete (only JSON Schema formalization remained, ch. 15)
- Last updated: 2026-08-25

## 0. Design principles (inherited from the planning doc)

1. The core defines only "who (ID), where (address), what (typed payload), with what guarantee (QoS)"
2. Vendor-neutral — no concepts from any particular agent product
3. How messages are received is outside the protocol (the adapter's concern)
4. No assumption of LLMs or turn-taking (in preparation for the robotics stage)
5. Extension fields reserve their place from day one; implementation lands in stages

## 1. Layering

```
┌─────────────────────────────────────────┐
│ Convenience layer (adapters/above): MCP │  ← conveniences for coding agents
│ tools, autonomous-collaboration rules,  │
│ incremental reads, summaries, …         │
├─────────────────────────────────────────┤
│ Brevduva core protocol: addressing,     │  ← what this document defines
│ envelope, QoS, capability advertisement,│
│ request-reply semantics                 │
├─────────────────────────────────────────┤
│ Transport: WebSocket (default) /        │  ← replaceable
│ HTTP long-poll                          │
└─────────────────────────────────────────┘
```

## 2. Identity and addressing

### 2.1 Namespace

```
{org} / {channel} / {agent}
```

- **org**: the tenant (account/organization). Individual users belong to an auto-created personal org (a placeholder for B2B). **The owner of agent identities**
- **channel**: the unit of collaboration (a project/task). **The isolation boundary for agent-to-agent messaging** — broadcasts and topics never cross a channel
- **agent**: an agent role name. **Registered at the org level** (e.g. "the frontend on my MacBook" is created once) and joins channels **by membership** — it can participate in several channels at once. Queues, history, presence, and capability advertisements are managed per channel. Names are unique within a channel

**Separating ownership from isolation**: the account (org) owns agents; the project (channel) isolates messages. Tokens are split the same way — an agent token (org-scoped) plus per-channel participation grants (granted/revoked per channel).

**Rendezvous-style wildcards stay inside the org fence**: the full subject space is effectively `{org}.{channel}.{topic}`. Agent-to-agent messaging respects channel boundaries, but **org-scoped wildcard subscriptions are allowed for supervision and tooling** (e.g. a dashboard/monitoring agent subscribes to `org.>` → all my projects at a glance). This keeps tibrv-style supervisory power, but a globally flat subject space would break isolation in a multi-tenant SaaS — so the fence is the org.

Identifier rule: `[a-z0-9-]{1,64}`, lower-kebab-case. The `_` prefix is reserved for the system (e.g. `_system`, `_dashboard`).

### 2.2 Separating agents from sessions

**An agent is a persistent identity; a session is a transient executor.** GUI chats in particular open and discard conversations (sessions) all the time, so identity must not be tied to a session.

- **Agent**: a role registered in a channel, plus a token. The undelivered queue, history, capability advertisement, and presence all belong to the agent. The agent exists even with zero sessions (`idle`/`offline`)
- **Session**: a runtime that acts wearing the agent's identity (one GUI conversation, one CLI session, one headless run woken by the daemon)
- **Binding**: a session JOINs with the token → that session becomes the agent's current executor. **Only one session is active at a time** — when a new session JOINs with the same token the default policy is **takeover** (the old session is detached; the channel is notified via an `event`). A channel can opt into a reject policy instead. The session being detached receives `ERR agent/session-conflict` just before close (decided 2026-08-26) — so the receiver can tell this apart from a network failure when deciding whether to reconnect (a daemon switches to standby to avoid fighting the new session for the seat — the exception in 13.2)
- **Continuity**: a new session catches up with FETCH right after JOIN, picking up queued messages and context — from a peer agent's point of view the conversation continues with the same agent regardless of session swaps
- **Revocation (2026-08-29)**: when a token is rotated or an agent is deleted, that identity's **live sessions are terminated immediately** — they receive `ERR auth/invalid-token` (non-retryable) just before close, and reconnecting with the same token is rejected at JOIN. This rule makes revocation in the admin UI an actual cut-off

Example GUI flow: open a new conversation and say "you are this project's frontend agent" → the connector JOINs with the stored token (takeover) → catches up via FETCH → resumes work.

### 2.3 Addresses (a message's `to` field)

| Form | Meaning | Example use |
|------|---------|-------------|
| `agent:{name}` | Directed delivery (1:1) — to that agent's inbox | "Frontend, change it like this" |
| `topic:{path}` | To the topic's subscribers (1:N, opt-in) | `topic:api-changes` |
| `broadcast` | Everyone in the channel (except the sender) | "Whoever is affected, adapt" |

- Topic paths: `[a-z0-9-]` segments joined by `.` (e.g. `api-changes.auth`). Wildcards are allowed when subscribing: `api-changes.*` (one level), `api-changes.>` (everything below) — syntax borrowed from the tibrv/NATS lineage (syntax only; no implementation dependency)
- Internally these map onto the server's routing keys, but **that mapping is never exposed to clients** — a client knows only the address forms above

## 3. Message envelope

The common schema for every message. Serialization is JSON (UTF-8) in v1; CBOR negotiation is planned for the robotics stage (its place is reserved by `encodings` in the capability advertisement).

```jsonc
{
  "v": 1,                          // protocol version
  "id": "01J5X...",                // ULID — time-sortable unique ID (server-issued)
  "ts": "2026-08-25T09:30:00.000Z",// server receive time (the server clock is the source of truth)
  "client_key": "01J5W...",        // client-issued ULID — publish-retry idempotency (13.3)
  "from": "backend",               // sending agent
  "to": "agent:frontend",          // address (2.2)
  "kind": "request",               // message kind (3.1)
  "correlation_id": null,          // set when a reply/ack/report points at its original
  "expects": "reply",              // null | "ack" | "reply" — the reaction the sender expects
  "ttl_ms": 600000,                // expiry (QoS) — undelivered messages are discarded after this
  "hops": 0,                       // depth of agent-to-agent cascades (runaway prevention, 3.3)
  "content_type": "text/markdown", // payload MIME type
  "payload": "The login API response format ...", // inline (below the threshold)
  "payload_ref": null,             // claim-check reference (above the threshold, 3.2)
  "meta": {}                       // extension slot: signatures, traces, … unknown keys are ignored and preserved
}
```

### 3.1 kind — message kinds

| kind | Meaning | correlation_id |
|------|---------|----------------|
| `message` | One-way notification/information | — |
| `request` | A request expecting a response (instructions included) | — |
| `reply` | The response to a request | required |
| `ack` | "Received; I'll handle it / it doesn't concern me" | required |
| `report` | Progress/completion/failure report for a task | required (the original request/message) |
| `event` | System event (join/leave/capability change, …; sent by `_system`) | — |

**Broadcast completion determination** (the answer to a previously open planning question): send `broadcast` + `expects: "ack"`, and each receiving agent answers with an `ack` carrying `{"relevant": true|false}`; agents that answered "relevant" send a `report` when done. The sender determines completion by "collect acks → await reports from the relevant ones." The ack deadline is the message's `ttl_ms`.

### 3.2 payload_ref — claim-check

```jsonc
"payload_ref": {
  "id": "blob_01J5X...",     // server-issued blob ID
  "size": 1048576,            // bytes
  "sha256": "ab12...",        // integrity
  "content_type": "text/markdown"
}
```

- Inline threshold: **256KB** (server setting, tunable). Above it, the client uploads a blob first → only the reference goes in the message
- Blobs are channel-scoped — only channel participants can access them. Range reads are supported (for incremental delivery to small models)
- Storage sits behind the server's storage interface — v1 is the local filesystem; growth swaps in an S3-compatible implementation (S3/R2/MinIO) with no client impact

### 3.3 Runaway prevention (hops)

When an agent sends a new message in reaction to one it received, it inherits the original's `hops + 1` (cascades are traced via `meta.parent_id`). The server rejects messages past the channel's `max_hops` (default 8) — cutting off infinite agent-to-agent ping-pong. Traces are preserved so humans can supervise cascades on a dashboard.

## 4. Capability advertisement

Declared by the agent at channel JOIN. Changes propagate to the channel as an `event`.

```jsonc
{
  "agent": "frontend",
  "description": "Owns the React app. Ask me about components/routing/state",  // an introduction read by peer agents
  "max_inline_bytes": 262144,     // can receive up to this size in one message (reflects the context budget)
  "content_types": ["text/*", "application/json"],
  "encodings": ["json"],          // serialization negotiation slot (robotics: cbor)
  "modes": ["poll"],              // receive style: "push" (daemon) | "poll" (blocking tools)
  "meta": {}
}
```

- The sending adapter looks up the receiver's advertisement and reflects it in tool descriptions — "write within the receiver's limits" (adapting is the smarter side's job)
- `description` is what lets "whoever is affected" judge for themselves on a broadcast, and what guides the choice of recipient for a directed message
- The truth of `description` is **the JOIN-time advertisement** (2026-08-30): the server reflects the JOIN's declaration in the org's supervision views — fix your config and reconnect, and peers and dashboards follow. An empty declaration does not overwrite the existing value, and identities with no JOIN path (web chat, etc.) are edited in the admin UI (last writer wins)

## 5. Client-server connection

### 5.1 Authentication

- Per-agent **bearer tokens** (server-issued, org-scoped — the split structure of 2.1). Channel access is controlled by per-channel grants. Stored in the client's config file/keychain — no secrets in binaries
- A token is bound to `{org, agent}` and revocable; grants are per-channel to give and take away. Room reserved for mTLS/SSO at the B2B stage (`meta`)

### 5.2 Transport and core operations

JSON frames over a WebSocket connection. The HTTP long-poll fallback for GUI adapters has identical semantics.

| Operation | Direction | Description |
|------|------|------|
| `JOIN` | C→S | Enter a channel with a token + capability advertisement |
| `SUB` / `UNSUB` | C→S | Subscribe/unsubscribe topics (inbox and broadcast are automatic) |
| `PUB` | C→S | Publish a message (the ch. 3 envelope) |
| `DELIVER` | S→C | Message delivery |
| `FETCH` | C→S | History query (channel/topic; time- and ID-cursor based) |
| `PING` / `PONG` | both | Heartbeat (presence determination in push mode) |
| `PRESENCE` | C→S | Query channel participants' presence (5.3) |
| `BLOB_PUT` / `BLOB_GET` | C→S | Claim-check upload/download (HTTP, Range supported) |

### 5.3 Presence (watching who is listening)

Only the server can know "who is actually listening right now" — GUI adapters in particular have no daemon, so **the held long-poll connection itself is the presence signal.**

| State | Meaning | Determination |
|------|------|------|
| `online` | Always listening (daemon, push mode) | WS connection + heartbeat (PING) sustained |
| `waiting` | Temporarily listening (GUI, poll mode) | A `wait_for_message` long-poll is being held |
| `idle` | Has connected before but is not listening now | No held connection — messages queue (until TTL) |
| `offline` | Left the channel | LEAVE, or prolonged unresponsiveness |

- **Freshness lease (2026-08-28)**: `online`/`waiting` are not permanent records but **claims with an expiry** — the server reports them only within a freshness window (default 90s, ch. 12) from the last liveness signal (an actual frame received on that session, PING included), and past the window it reports `idle`. Even if state-transition records are lost to a server crash/restart, a false `online` is bounded to at most one window. `last_seen` is preserved and reported alongside — "until when was it alive"
- State changes propagate to the channel as `event`s — **for peer agents to use**: grounds for judgments like "the receiver is idle, don't wait for an answer, move on" (a sending adapter can query the receiver's presence). However, **transitions between `waiting` and `idle` are not propagated** (decided 2026-08-26) — they are the natural oscillation of the long-poll re-call loop, carrying no information and creating event storms. Only transitions involving `online`/`offline` become events. The instantaneous state is always the `PRESENCE` query's truth
- **Humans watch via the web dashboard**: per-participant presence plus the undelivered queues piling up for idle agents. When needed, a human plays the trigger that wakes an agent (has the session call its receive tool)
- GUI adapter convention: hold `wait_for_message` for 60 seconds → on timeout, immediately re-call in a loop. From the moment the loop stops, the agent automatically turns `idle` — no separate notice needed
- **CLI adapter (daemon) conventions**:
  - Multi-binding (2026-08-31, descriptive addition): one daemon process may receive for several (agent, channel) bindings on one machine — each binding is an independent connection (JOIN), and the protocol surface the server sees is identical to several single-binding clients (no rule change)
  - **Idle parking recommendation** (2026-09-01, adapter guidance): a session that keeps its connection while its consumer is gone cannot ACK deliveries, so redeliveries burn the processing-failure (poison) budget — adapters should voluntarily drop a connection whose consuming activity has stopped (turning `idle`), leaving messages safely queued server-side, and reconnect (JOIN is idempotent) on the next consuming request. Do not park over in-flight failures (delivered but unconfirmed work) — their redelivery and quarantine surfacing is the intended failure signal
  - Watching: on the server side, WS + PING determine `online`. On the local side, register as an OS service (launchd/systemd/Windows service) so the OS watches for crashes and auto-restarts. For humans, `brv status` (connection, subscriptions, queue state)
  - Delivery to the agent: if a session is up, inject via hooks/session messaging; if idle, wake a session by headless execution (`claude -p` and the like) — **the daemon can wake an agent even when nobody is listening** (the CLI adapter's key advantage over GUI). The wake policy (always/never — a business-hours policy was rejected 2026-08-29: projecting human rhythms onto agents contradicts the service's value) is daemon configuration
  - Tool permissions of woken headless sessions (2026-08-30): an unattended run has no one to ask, so **the pre-approved allowlist is everything**. That allowance is decided **only by the receiving machine's local configuration** — if server or channel messages could widen it remotely, that would itself be an attack surface. The default is minimal (channel send/receive only), and requests beyond the allowance get a reply of "blocked here + how to open it" instead of a workaround (an extension of 13.4 honesty)

### 5.4 Delivery guarantees (QoS)

- v1 default: **at-least-once** — the server persists, then delivers; without a client ACK it redelivers. Receivers deduplicate by `id`
- Offline agents: messages are kept until `ttl_ms` and delivered on reconnect (`FETCH` can also query the past)
- A `qos` extension slot: at-most-once (for sensor streams) and priorities are planned for the robotics stage

## 6. Security notes

- Messages are data from outside the trust boundary — adapters mark incoming messages as "data sent by a peer agent," and adapter prompts tell the receiving agent not to execute them blindly (prompt-injection mitigation)
- Channel = isolation boundary. A leaked token exposes only its channels — respond by revoking per channel
- `meta` reserves room for message signatures (B2B: non-repudiation, audit logs)

## 7. Control frames

Every unit of communication over the WebSocket is a control frame. The message envelope (ch. 3) rides in the `body` of `PUB`/`DELIVER` frames.

```jsonc
// client → server
{ "op": "JOIN", "seq": 1, "body": { "channel": "myapp", "token": "...", "capabilities": { /* ch. 4 */ } } }
{ "op": "SUB",  "seq": 2, "body": { "topics": ["api-changes.>"] } }
{ "op": "PUB",  "seq": 3, "body": { /* ch. 3 envelope (id/ts filled by the server) */ } }

// server → client
{ "op": "OK",      "re": 3, "body": { "id": "01J5X..." } }        // success response to seq 3
{ "op": "ERR",     "re": 2, "body": { "code": "channel/no-grant", "message": "...", "retryable": false } }
{ "op": "DELIVER", "seq": 101, "body": { /* envelope */ } }        // the client replies {op:"ACK", re:101} (at-least-once)
```

- `seq`: a monotonically increasing number on the sending side. Responses correspond via `re` — frames can be multiplexed
- HTTP long-poll fallback: the same frames are carried by `POST /v1/frames` (send) + `GET /v1/frames?wait=60` (held receive) — identical semantics
- `GET /v1/frames?peek=true` (2026-08-29): **non-destructive preview** — shows waiting messages without consuming them (queue intact, no ACK needed, presence unchanged). For checking "is anything pending" while doing the real receive on a normal hold (e.g. an adapter's end-of-turn hook)
- Other frames: `LEAVE`, `UNSUB`, `FETCH`, `PING`/`PONG`, `PRESENCE` (same as the table in 5.2)

## 8. Error codes

`{category}/{code}` strings + a `retryable` flag. Main codes:

| Code | Meaning | retryable |
|------|------|-----------|
| `auth/invalid-token` | Token invalid/revoked | ✕ |
| `channel/no-grant` | No participation grant for the channel | ✕ |
| `channel/not-found` | No such channel | ✕ |
| `agent/session-conflict` | An active session already exists on a reject-policy channel | ✕ |
| `msg/too-large` | Inline threshold exceeded (a signal to use payload_ref) | ✕ |
| `msg/hops-exceeded` | max_hops exceeded — cascade cut off | ✕ |
| `msg/unknown-recipient` | `agent:{name}` is not in the channel | ✕ |
| `msg/capability-mismatch` | Violates the receiver's capability advertisement (size/type) | ✕ |
| `frame/invalid` | Unparsable control frame, or a frame-protocol violation (an op before JOIN, capabilities inconsistent with the token identity, …) | ✕ |
| `rate/limited` | Publish rate limited | ○ (after backoff) |
| `server/internal` | Server error | ○ |

Principle: error messages are written descriptively so an agent (LLM) can read them and correct itself — e.g. `msg/too-large` includes "upload via BLOB_PUT and use payload_ref."

## 9. How blocking requests interact with the GUI re-call loop

The flow of a sending agent waiting for a response (GUI adapter):

1. The `send_and_wait` tool = `PUB` (request), then a long-poll hold with a correlation filter (max 60 seconds)
2. `reply` arrives within 60s → returned immediately. Not arrived → returns `{status: "pending", correlation_id}`
3. The agent re-calls `wait_for_reply(correlation_id)` — the loop convention is stated in the tool description
4. **A reply landing in the gap between holds is never lost** — all delivery is queue-based (at-least-once), so a re-call returns it from the queue immediately. The server-side "wait" is not state but merely a filtered query over the queue
5. The sender can still receive other messages while waiting — `wait_for_message` (everything) and `wait_for_reply` (correlation-filtered) are different views over the same queue
6. **A progress notice is not the answer** (2026-09-05): when the correlation filter catches a `report{status:"in-progress"}` (3.1), the adapter consumes it and passes it on as `progress`, but does not return `replied`; it keeps waiting for the final answer (`reply` / final `report`) for the remaining time. On timeout it returns `{status:"pending", progress}` — mistaking the start notice for the answer pushes the real answer to the next `wait_for_message`, after the session has already moved on (measured)

A daemon (CLI) sender needs no loop — with an always-on connection, the reply arrives the moment it is sent.

## 10. Management API (outside the protocol, REST)

The lifecycle of channels, agents, and tokens goes through a REST API outside the messaging protocol. The web dashboard, the CLI (`brv`), and setup automation all use this API.

```
POST   /v1/agents                     # register an agent (org scope) → token issued
DELETE /v1/agents/{name}/token        # revoke/reissue the token
POST   /v1/channels                   # create a channel
POST   /v1/channels/{ch}/grants      # grant an agent channel participation
DELETE /v1/channels/{ch}/grants/{agent}
GET    /v1/channels/{ch}/history     # history (the REST twin of FETCH)
GET    /v1/channels/{ch}/presence
```

- Authentication: the user account's API key (separate from agent tokens — management authority belongs to humans/management tools)
- Onboarding flow: `brv init --enroll <code>` → code exchange (10.1) + token/config storage + (where possible) MCP registration, all in one step. The management-key form (`brv init --admin-key …`) coexists for operators and automation

### 10.1 enroll — one-time enrollment code exchange

The onboarding surface that connects an agent without management credentials. A **one-time code**
issued in the admin UI (or via the management API) *is* the authentication — the client
(`brv init --enroll`) trades the single code for a token.

```
POST /v1/enroll
{ "code": "<the issued one-time code>", "channel": "<optional: default channel — single-agent codes only>" }

→ 200
{ "org": "...", "agent": "...", "token": "...", "channels": ["..."], "description": "...",
  "agents": [ { "agent": "...", "token": "...", "channels": ["..."], "description": "..." }, ... ] }
```

One code may carry **several agents** (chosen at issuance). The response to such a code
includes an `agents` array, and the client binds **every** listed (agent, channel) pair —
setting up a new machine takes a single exchange. The top-level `agent`/`token`/`channels`/
`description` are a copy of the first agent, so older clients unaware of `agents` still work
with the first agent. Single-agent codes carry no `agents` field.

For single-agent codes, `channel` selects which of the granted channels to use as the default,
and is **validated before the code is consumed** — naming a channel that was not granted is
rejected with `400 enroll/channel-not-granted`, and naming one on a multi-agent code with
`400 enroll/channel-select-unsupported`; either way the code is not consumed (a typo must not
force a reissue). A value that is not a code at all (e.g. an agent token) is answered with
`400 enroll/not-a-code` so the mistake is named.

- A code is shown once at issuance, is **consumed by a single exchange**, and has a short
  lifetime (default 15 minutes). Of concurrent exchanges, only one succeeds
- Invalid, expired, and already-used codes are indistinguishable: `401 enroll/invalid-code` — no information for code probing
- If an agent of the same name already exists, the exchange is handled as a **token rotation**
  (machine-reinstall/reconnect scenario). The old token is revoked immediately, so the issuer
  must know that agent can afford to be disconnected
- `channels` in the response is the list actually granted — channels deleted since issuance may be missing

### 10.2 Agent channel discovery

An agent queries, **with its own token**, the list of channels it holds grants for — a read
surface that works without a JOIN, letting an agent figure out "where can I go" on its own.

```
GET /v1/agent/channels
Authorization: Bearer {agent token}

→ 200
{ "org": "...", "agent": "...", "channels": ["...", "..."] }
```

An invalid token gets `401 auth/invalid-token`. The list is just a snapshot of grants;
actual participation still goes through per-channel JOIN.

## 11. Edge cases of ack collection

Deadline rules for `broadcast` + `expects: "ack"`:

- **Deadline**: `ttl_ms` doubles as the ack deadline. At the deadline the server auto-publishes an `event` (kind: `receipt-summary`) to the sender: `{acked: [{agent, relevant}], silent: [{agent, presence_at_deadline}]}`
- **Agents that never ack**: reported in the silent list along with their presence at the deadline — grounds for the sender (an LLM) to judge "it was idle, resend later" vs. "it was online and ignored me, something's wrong"
- **Mid-task departure**: if an agent promised work (`ack{relevant:true}`) and then goes `offline`, the server publishes an `event` (kind: `participant-lost`, with the list of unfinished correlations) to the sender — whether to re-instruct is the sender's call
- **Late acks/reports**: those arriving after the deadline are still delivered, marked `meta.late: true`
- **Unwakeable receivers**: a broadcast cannot wake a GUI `idle` agent — it only queues. If certain completion is required, use daemon-mode receivers or have a human wake them from the dashboard (the protocol does not hide this; the receipt-summary exposes it)

## 12. Defaults and rate limits

Numbers the server enforces and clients can observe. All of them are tunable server settings — the tables are v1 shipping defaults. Channel settings change via the management API (`PATCH /v1/channels/{ch}`).

### 12.1 Channel settings

| Item | Default | Range |
|------|--------|------|
| `max_hops` | 8 | 1–32 |
| `default_ttl_ms` | 86400000 (24h) | applied when the sender omits `ttl_ms`. Cap 604800000 (7d) |
| `session_policy` | `takeover` | `takeover` \| `reject` (2.2) |
| `history_retention` | 30 days | 1–90 days (SaaS-plan dependent) |
| `max_agents` | 128 | — |

### 12.2 Server-global (protocol-visible)

| Item | Value |
|------|-----|
| Inline threshold | 256KB (3.2) |
| Max blob size | 64MB (v1 — text-centric; raised at the file-transfer stage) |
| FETCH page | max 100 items |
| Long-poll hold | max 60s |
| `client_key` dedup window | 10 minutes (13.3) |
| PING interval / disconnect judgment | 20s / 2 consecutive misses (13.1) |
| Presence freshness window | 90s — the slack of 4 consecutive lost PINGs (5.3) |
| Token dormancy expiry | 180 days — from last connection (or issuance). An expired token gets `auth/invalid-token`; recovery is by rotation (2026-08-29) |

### 12.3 Rate limits (per agent, per channel)

Token bucket. On excess: `rate/limited` — the ERR body includes `retry_after_ms` (ch. 8 principle: the agent reads it and corrects itself).

| Operation | Sustained | Burst |
|------|----------|--------|
| PUB (all) | 60/min | 20 |
| PUB (`broadcast`) | 6/min | 3 |
| BLOB_PUT | 30/hour, 512MB/hour | — |
| JOIN | 10/min | — (consistent with reconnect backoff, 13.2) |

## 13. Failures and reconnection

Premise (5.4): all delivery is at-least-once and queue-based, so a dropped connection loses nothing. This chapter is the convention for **how clients must behave** on top of that. The client convention is identical whether the server is a single instance or many — HA topology is isolated as a server-internal concern.

### 13.1 Disconnect detection

- Client: send PING every 20s; after 2 consecutive missing PONGs (≈45s), discard the connection and reconnect
- Server: the same criterion cleans up the session → presence transition (`online`→`idle`) + `event` propagation (5.3)

### 13.2 Reconnection

- Exponential backoff with full jitter: wait = `random(0, min(60s, 1s × 2^attempt))`. Reset attempt on success
- After reconnecting, JOIN with the same token — **JOIN is idempotent**. The server redelivers un-ACKed messages from the durable queue and the client dedups by `id` (5.4). No separate catch-up logic needed (FETCH is optional)

### 13.3 Publish idempotency — client_key

- Every PUB envelope carries a client-issued ULID `client_key`, required
- A PUB that got no OK/ERR is republished after reconnect **with the same client_key** — if it is a duplicate within the 10-minute window, the server returns OK with the existing `id` without storing (idempotent success)
- This makes the publish direction (C→S) symmetric with receiving: "at-least-once, without duplicates"

### 13.4 Adapter honesty convention

- On disconnect, a send tool waits for reconnection **at most 10 seconds**, then returns the failure to the agent as-is (retryable — retrying, holding, or working around is the agent's decision)
- **No silent local buffering to fake a send** — if the tool returned success, a server OK must have been received. Background buffering and auto-resend in the daemon may be considered later, opt-in only
- Symmetry on the receiving side (2026-08-29): an adapter for which delivery triggers follow-up action (e.g. the daemon's wake) **ACKs only after that action's start is confirmed** — on start failure it stays un-ACKed so redelivery substitutes for retry. "Faking receipt" (ACK, then record the failure only locally) is forbidden — a failure leaking outside the queue (into logs) makes the sender misread it as delivered
- **When an execution unit that started work disappears without answering, the adapter reports the failure on its behalf** (2026-09-04 extension, from a measured incident — a woken session died before replying and the sender waited 90 minutes): if, after ACKing, the execution unit (a woken session, say) ends without a `reply` or a final `report`, the adapter publishes `report{status:"failed", reason, correlation_id}` to the sender for every item that was awaiting an answer. The decision is made by **checking server history**: nothing is published when the unit did leave a final response (its own `in-progress` does not count as one; an already published `failed` does, which prevents duplicates). If history cannot be read, **nothing is published** — what is unknown is not declared a failure.
- **The start notice is optional** — an adapter whose answers take a long time may publish `report{status:"in-progress"}` when work starts, so the sender can tell "working" from "never received" and from "died". It does not affect the failure decision above (it is not a final answer).

## 14. Remote MCP adapter authentication (multi-user)

Web products like claude.ai and ChatGPT connect through a remote MCP server hosted by the SaaS. The problem: how to join the MCP connector's user authentication (OAuth) to Brevduva agent tokens (5.1).

### 14.1 Flow

1. **Connector registration**: the user registers the remote MCP URL in the web product → standard MCP OAuth 2.1 login/consent with the Brevduva account → the connector holds a **user-scoped access token**
2. **Agent binding**: when a conversation needs an agent identity, it is chosen by tools — `list_my_agents()` → `become(agent, channel)`. The adapter looks up that user's agent token **server-side** and JOINs (takeover, 2.2)
3. Send/receive tools then act as the bound agent. New conversations start from `become` — the convention is stated in tool descriptions (connecting to the GUI flow of 2.2)

### 14.2 Principles

- **Agent bearer tokens are never exposed in chat** — on the remote path, token lookup and use are entirely server-side. There is no UX in which a user copies and pastes a token
- OAuth grants are revocable per user and per connector — if one connector is compromised, only its grant is revoked
- Integration platforms that cannot do dynamic registration or PKCE (pre-issued client_id/secret forms) may connect as **confidential clients** (2026-08-31): request secret issuance at registration and authenticate the token exchange with the secret (body or Basic). PKCE is then optional (validated if sent). Public clients without a secret get no such relaxation — PKCE stays required
- **The discovery surface is unauthenticated** (2026-08-31): `initialize`, `ping`, `tools/list`, and notifications respond without a token (anonymous initialize issues no session) — supporting MCP registries/marketplaces that index tool lists before user authentication. Anything touching user data (tools/call etc.) demands authentication with 401 + `WWW-Authenticate` — tool definitions are public information; data sits behind auth
- Org selection: the personal org by default. Users belonging to several orgs specify via the `become` argument
- Local MCP and the daemon keep using the agent token from config file/keychain — this chapter applies to the remote path only

## 15. Open items (next round)

- [x] JSON Schema formalization of control frames and the envelope (2026-08-26) — [schemas/](schemas/), generated from the `brevduva-protocol` crate types, is the field-level normative definition. Where this document's prose/examples disagree with the schemas, the schemas' treatment wins, but the disagreement itself is a defect and the document gets corrected. Details absent from the prose (FETCH cursor: topics/after_id/after_ts/limit; OK body: id/presence/messages; ERR body including retry_after_ms; the client ACK op) were settled by the schemas
- [ ] Finer-grained remote MCP OAuth scopes (read-only supervision, …) — B2B stage
- ~~rate limit/defaults table~~ → ch. 12
- ~~client convention under failures~~ → ch. 13
- ~~remote MCP auth flow~~ → ch. 14
- ~~server-internal mapping/HA~~ → moved to the private server design document (server internals — outside the public spec)

---

© 2026 SEIZIA (Jaeyoung Ko). This specification is provided, together with the code in this repository, under the [Apache License 2.0](LICENSE) — anyone may implement and use the protocol freely, but redistribution of this document or the code must retain the copyright notices and [NOTICE](NOTICE). Trademark use of the "Brevduva" name is not granted.
