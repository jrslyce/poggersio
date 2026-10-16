# tickwire + poggersio — Build Handoff

> **Audience:** one coordinator session and two worker agents (Agent S = server, Agent C = client).
> **Supersedes:** `CLIENT_HANDOFF.md` (SpacetimeDB client plan). The SpacetimeDB module in `server/` is the
> **reference implementation of the game rules** until parity is reached, then it is deleted (milestone M6).

---

## 0. TL;DR

We are replacing SpacetimeDB with our own **reusable real-time game networking stack** called **tickwire**,
then rebuilding poggersio on top of it.

- **tickwire (reusable, game-agnostic)**
  - Rust server framework: WebTransport (HTTP/3, QUIC datagrams + streams) with automatic WebSocket fallback,
    sessions, a binary codec, fixed-tick rooms, matchmaking, spatial grid, metrics.
  - TypeScript client SDK: connect (WebTransport → WebSocket fallback), the same binary codec, fragment
    reassembly, RTT/clock sync, snapshot interpolation buffer.
- **poggersio (first game on tickwire)**
  - Rust game crate: port of `server/src/lib.rs` game rules (with known bugs fixed) implementing tickwire's `Game` trait.
  - Browser client: Vite + TypeScript + Canvas 2D.

Why: SpacetimeDB billed every 50 ms tick and every row write as a database transaction and broadcast whole tables
to every subscriber. tickwire keeps state in memory, ticks per room, and sends each client only its viewport in a
compact binary format. Target: **~3,500–5,000 concurrent players per 8-core box**, 10k players on 2–3 boxes.

---

## 1. Goals and non-goals

### Goals
1. A **game-agnostic** framework that future WebTransport games reuse without modification: a new game implements
   one Rust trait and one TS message handler set.
2. **WebTransport first, WebSocket fallback**, transparent to game code.
3. poggersio at feature parity with the SpacetimeDB version (see §9), plus fixes listed in §9.4.
4. Measurable performance: load-test harness and the targets in §11.
5. Two agents can work **in parallel from day one** against a frozen wire contract (§5) and shared test vectors.

### Non-goals (v1)
- Client-side prediction / rollback (snapshot interpolation only; agar.io-style games don't need prediction).
- Delta-compressed snapshots against acked baselines (message ID reserved; v2).
- Multi-box matchmaking service (v1 = single process with many rooms; §7.6 exposes the hooks a future lobby needs).
- Reconnect-and-resume a session (v1: disconnect = player dies, same as today).
- Accounts / auth providers (Hello carries an opaque token field for later).
- Real Twitch API integration, mobile/touch controls.

---

## 2. Team, ownership, and coordination model

### 2.1 Roles

| Role | Who | Owns (write access) | Never edits |
|---|---|---|---|
| **Coordinator** | The orchestrating session ("web coordinator") | `protocol/`, `COORDINATION.md`, root `Cargo.toml` workspace members, `.github/workflows/`, `README.md`, this file | Implementation code (except to unblock a merge conflict) |
| **Agent S** (server) | Worker session #1 | `crates/**`, `games/poggersio/server/**`, `tools/loadtest/**` | `packages/**`, `games/*/client/**`, `protocol/**` |
| **Agent C** (client) | Worker session #2 | `packages/**`, `games/poggersio/client/**`, `examples/dots/client/**` | `crates/**`, `games/*/server/**`, `protocol/**` |

Shared example `examples/dots/` is split: `examples/dots/server` → Agent S, `examples/dots/client` → Agent C.

### 2.2 The contract is the coordination mechanism

The two agents never wait on each other's code to *start* work. They both build against:

1. `protocol/SPEC.md`: the wire format (copied from §5 and §9.3 of this doc at M0, then the source of truth).
2. `protocol/vectors/*.json`: golden encode/decode vectors. **Both codecs must pass every vector in CI.**
3. `protocol/constants.json`: message type IDs, protocol version, limits. Both sides generate or hand-mirror constants
   from this file, and a CI test asserts they match.

**Contract change process** (the only way the protocol changes):
1. Agent writes a request in `COORDINATION.md → Contract change requests` (what, why, which messages).
2. Coordinator approves or rejects, edits `protocol/`, bumps `PROTOCOL_VERSION` if wire-incompatible, adds or updates vectors,
   merges to `main`, and messages both agents.
3. Both agents rebase/merge `main` and update their codecs. Vectors fail until they do.

### 2.3 Branches and PRs

- Agent S works on `tw/server/<milestone>-<topic>` (e.g. `tw/server/m1-codec`), Agent C on `tw/client/<milestone>-<topic>`.
- Small PRs into `main`, one milestone topic each. Coordinator reviews and merges. CI must be green.
- Agents merge `main` into their branch at least at every sync point (§3). No force-pushes to `main`.

### 2.4 `COORDINATION.md` (status board, created by coordinator at M0)

```markdown
# Coordination board
Protocol version: 1   |   Current milestone: M1   |   Next sync point: S1

## Agent S status
- [ ] M1.S1 proto codec passes vectors
...
Blockers: none
Last update: <date> <one line>

## Agent C status
- [ ] M1.C1 TS codec passes vectors
...
Blockers: none
Last update: <date> <one line>

## Contract change requests
| # | From | Request | Status | Resolution |

## Decisions log
| Date | Decision | Why |
```

Agents update their own section **at the end of every work session** and whenever they hit a blocker.
They do not edit each other's sections.

### 2.5 Messaging between sessions

When the coordinator runs as a Claude Code cloud session, it uses `create_session` to launch each worker with the
kickoff prompt in Appendix A/B, and `send_message` to notify agents of merged contract changes and sync points.
Workers report back by updating `COORDINATION.md` and opening PRs (the coordinator watches PRs). Any other
orchestrator works the same way: the repo files above are the shared state, and messages are only nudges.

---

## 3. Milestones and sync points

Each milestone lists parallel tracks. **Sync points (S#)** are integration gates: the coordinator runs the gate test,
and both agents must merge `main` before starting the next milestone.

### M0: Foundation (Coordinator only, before workers start)
- Restructure the repo to §4 layout (leave `server/` untouched as the reference).
- Write `protocol/SPEC.md`, `protocol/constants.json`, `protocol/vectors/framework.json`, `protocol/vectors/poggersio.json`
  (vectors per §5.9).
- Root Cargo workspace, root `package.json` with npm workspaces, `.github/workflows/ci.yml` (§12).
- Create `COORDINATION.md`. Pin dependency versions (latest stable at scaffolding time) and record them in the decisions log.
- Launch Agent S and Agent C.

### M1: Codecs + WebSocket handshake
| Agent S | Agent C |
|---|---|
| `tickwire-proto`: Writer/Reader, envelope, framework messages, passes `framework.json` vectors | `@tickwire/client` codec: Writer/Reader, envelope, framework messages, passes `framework.json` vectors |
| `tickwire-transport`: WebSocket listener (axum), Hello/Welcome/Reject, Ping/Pong, Disconnect, per-session outbound queue | WebSocket transport, `connect()` with Hello → Welcome, Ping/RTT, close handling |
| `tickwire` facade with `ServerBuilder` and a built-in echo game for tests | Unit tests with an in-memory fake transport |

**S1 gate:** a Vitest integration test in `packages/tickwire-client` connects over WebSocket to `cargo run -p tickwire --example echo`,
completes the handshake, round-trips 1,000 reliable echo messages, and measures RTT.

### M2: WebTransport
| Agent S | Agent C |
|---|---|
| WebTransport listener (`wtransport`): control stream framing, datagrams, fragmentation (§5.6), dev self-signed cert + `/dev/cert-hash` endpoint | WebTransport transport: control stream, datagrams, fragment reassembly, `serverCertificateHashes` for dev |
| Same session abstraction for both transports; game code can't tell them apart | `transport: "auto"` races WT with a timeout, then falls back to WS; exposes `conn.transport` |

**S2 gate:** Playwright (Chromium) test page from `packages/tickwire-client/test/e2e` connects to the echo server
three ways (`auto`, forced `webtransport`, forced `websocket`) and passes: handshake, 1,000 reliable echoes, 1,000 unreliable
echoes with ≥ 95% delivered on localhost, one 8 KB unreliable message delivered via fragmentation.

### M3: Rooms + reference game ("dots")
| Agent S | Agent C |
|---|---|
| `tickwire-rooms`: `Game` trait (§7.3), room runtime, fixed tick, matchmaking (QuickJoin), room lifecycle, backpressure, spatial grid, `/status` + `/metrics` | `SnapshotBuffer` interpolation, server-clock estimation, `joinRoom()` API |
| `examples/dots/server`: each player is a dot moving toward the mouse, 20 Hz, viewport snapshots | `examples/dots/client`: Canvas render with interpolation; the template future games copy |

**S3 gate:** two browser tabs see each other's dots move smoothly; killing one tab removes its dot within 1 tick;
an empty room closes after `empty_room_ttl`.

### M4: poggersio game (largest milestone, fully parallel)
| Agent S | Agent C |
|---|---|
| `games/poggersio/server`: port all rules from `server/src/lib.rs` (§9), fix §9.4 bugs, implement messages §9.3, bots, leaderboard, kill feed, emotes incl. burst AoE | `games/poggersio/client`: login, render, HUD, input, emotes, death/respawn (§10). Develop against `FakeServer` (§10.6) until S4 |
| Unit tests for game rules (eat rule, split, merge, virus, decay, burst) | Unit tests for decoders against `poggersio.json` vectors |

**S4 gate:** coordinator plays against the real server in Chromium: the full §10.8 acceptance checklist passes on both transports.

### M5: Load, hardening, polish
| Agent S | Agent C |
|---|---|
| `tools/loadtest`: headless bot clients (WT and WS), ramp, report | Reconnect UX (back to login on disconnect), error toasts, transport indicator, perf HUD (F3) |
| Rate limits, validation, fuzz the decoders (`cargo fuzz` or proptest), meet §11 targets | Canvas perf: 60 fps with 150 visible cells on a mid laptop; offscreen food layer |

**S5 gate:** load test of 2,000 bot clients on one 8-core box meets §11; report committed to `docs/perf/`.

### M6: Ship (Coordinator)
- `docs/DEPLOY.md` (§13), delete `server/` (SpacetimeDB) and `CLIENT_HANDOFF.md`, update `README.md`.
- Tag `tickwire-v0.1.0`.

---

## 4. Repository layout (target)

```
poggersio/
├── Cargo.toml                    # [workspace] members = crates/*, games/*/server, examples/*/server, tools/*
├── package.json                  # npm workspaces: packages/*, games/*/client, examples/*/client
├── COORDINATION.md
├── TICKWIRE_HANDOFF.md           # this file
├── protocol/
│   ├── SPEC.md                   # wire contract (source of truth after M0)
│   ├── constants.json            # message IDs, limits, PROTOCOL_VERSION
│   └── vectors/
│       ├── framework.json
│       └── poggersio.json
├── crates/
│   ├── tickwire-proto/           # codec, envelope, framework messages (no I/O, no tokio)
│   ├── tickwire-transport/       # WT + WS listeners, sessions, fragmentation, TLS/dev certs
│   ├── tickwire-rooms/           # Game trait, room runtime, matchmaker, spatial grid
│   └── tickwire/                 # facade: re-exports + ServerBuilder + HTTP (/status, /metrics, /dev/cert-hash)
├── packages/
│   └── tickwire-client/          # @tickwire/client (TS, ESM, zero runtime deps)
├── examples/dots/{server,client}/
├── games/poggersio/
│   ├── server/                   # bin crate: poggersio-server
│   └── client/                   # Vite + TS + Canvas 2D
├── tools/loadtest/               # bin crate: tickwire-loadtest
├── docs/                         # DEPLOY.md, perf reports
└── server/                       # LEGACY SpacetimeDB module: read-only reference, deleted at M6
```

Rule of reuse: **nothing under `crates/` or `packages/` may mention poggersio**, cells, food, emotes, or any game concept.
CI greps for it (§12).

---

## 5. Wire protocol (framework layer)

### 5.1 Conventions
- All integers **little-endian**. Floats are IEEE-754 `f32` LE.
- `varu32`: unsigned LEB128, max 5 bytes. Used for counts and IDs where noted.
- `str`: `u16` byte length + UTF-8 bytes. Decoders reject invalid UTF-8 and lengths over the field's limit.
- `bytes`: `u32` length + raw bytes.
- Every message is an **envelope**: `u8 msg_type` followed by the body. Envelope max size: **64 KiB** (`MAX_MESSAGE`).
- Message type space: `0x00–0x3F` framework (reserved), `0x40–0x7F` game client→server, `0x80–0xFF` game server→client.
- Unknown framework type → protocol error → Disconnect(`PROTOCOL_ERROR`). Unknown game type → passed to the game, which decides.

### 5.2 Channels

| Channel | WebTransport | WebSocket |
|---|---|---|
| **Reliable** (ordered) | One bidirectional stream opened by the client right after the session is established (the "control stream"). Each envelope is prefixed with `u32` LE length. | Each binary WS message = exactly one envelope. |
| **Unreliable** (unordered, may drop) | QUIC datagrams. One datagram = one envelope, or one `Fragment` (§5.6). | Sent as ordinary WS binary messages (reliable in practice). Never fragmented. |

The session API exposes `send_reliable` / `send_unreliable` identically for both. Game code never sees the transport.

### 5.3 Endpoints
- WebTransport: `https://<host>:<wt_port>/tw` (default dev port 4433, UDP).
- WebSocket: `ws(s)://<host>:<http_port>/tw/ws` (default dev port 8080, TCP).
- HTTP on the same port as WS: `GET /status`, `GET /metrics`, `GET /dev/cert-hash` (dev mode only).

### 5.4 Session lifecycle
1. Client connects (WT session + control stream, or WS).
2. Client → `Hello` (reliable) within 5 s, or the server closes the connection.
3. Server → `Welcome` or `Reject`.
4. Client → `JoinRoom` (reliable). Server → `RoomJoined` or `Error`.
5. Game traffic. Either side may send `Ping`; the peer answers `Pong` immediately.
6. Either side → `Disconnect`, then close. Server idle timeout: 15 s without any inbound message. Clients send `Ping` every 2 s.

### 5.5 Framework messages

| ID | Name | Dir | Channel | Body |
|---|---|---|---|---|
| `0x01` | Hello | C→S | R | `u16 protocol_version`, `str game_id` (≤32), `str client_build` (≤32), `str token` (≤512, may be empty) |
| `0x02` | Welcome | S→C | R | `u32 session_id`, `u16 tick_rate_hz`, `u32 server_tick`, `u16 max_datagram` (0 on WS), `str server_build` |
| `0x03` | Reject | S→C | R | `u8 reason` (§5.7), `str message` (≤256) |
| `0x04` | Ping | both | U | `u32 nonce`, `u32 sender_time_ms` (sender's monotonic ms, wraps) |
| `0x05` | Pong | both | U | `u32 nonce`, `u32 echoed_time_ms`, `u32 server_tick` (0 when sent by client) |
| `0x06` | SnapshotAck | C→S | U | `u32 tick` (**reserved for v2 delta**; servers ignore in v1) |
| `0x07` | Fragment | both | U (WT only) | `u16 group`, `u8 index`, `u8 count`, then chunk bytes to end of datagram |
| `0x08` | Disconnect | both | R | `u8 reason` (§5.7), `str message` (≤256) |
| `0x09` | JoinRoom | C→S | R | `str room_kind` (≤32, game-defined, `""` = default), `bytes payload` (game-defined join data, ≤4096) |
| `0x0A` | RoomJoined | S→C | R | `u32 room_id`, `bytes payload` (game-defined, e.g. config + initial state) |
| `0x0B` | LeaveRoom | C→S | R | empty |
| `0x0C` | RoomLeft | S→C | R | `u8 reason` (§5.7) |
| `0x0D` | Error | S→C | R | `u16 code`, `str message` (≤256). Non-fatal error for the game to show (e.g. "name too long"). |

### 5.6 Fragmentation (WebTransport datagrams only)
- `max_datagram` = the QUIC session's current max datagram payload (from `wtransport`), sent in `Welcome`. If unknown, use 1,100.
- If an unreliable envelope exceeds `max_datagram`, the sender splits it into chunks of `max_datagram − 5` bytes and wraps each
  in a `Fragment` envelope (5 = 1 type + 2 group + 1 index + 1 count). Max 32 fragments (≈ 35 KB). Larger unreliable
  messages are a sender bug: debug builds panic, release builds send on the reliable channel instead and count a metric.
- `group` increments per fragmented message, wrapping at 65,535. The receiver keeps at most **4 incomplete groups**, evicts the oldest,
  and drops a group after 500 ms. A completed group decodes as a normal envelope.

### 5.7 Reason and error codes

`Reject` / `Disconnect` / `RoomLeft` reasons (`u8`):

| Code | Name |
|---|---|
| 1 | `VERSION_MISMATCH` |
| 2 | `UNKNOWN_GAME` |
| 3 | `SERVER_FULL` |
| 4 | `PROTOCOL_ERROR` |
| 5 | `RATE_LIMITED` |
| 6 | `IDLE_TIMEOUT` |
| 7 | `KICKED` |
| 8 | `SERVER_SHUTDOWN` |
| 9 | `CLIENT_LEFT` |
| 10 | `ROOM_CLOSED` |
| 11 | `AUTH_FAILED` |

`Error.code` (`u16`): `1–999` framework, `1000+` game-defined.

### 5.8 Limits (also in `protocol/constants.json`)

| Name | Value |
|---|---|
| `PROTOCOL_VERSION` | 1 |
| `MAX_MESSAGE` | 65,536 bytes |
| `MAX_FRAGMENTS` | 32 |
| `HELLO_TIMEOUT_MS` | 5,000 |
| `IDLE_TIMEOUT_MS` | 15,000 |
| `PING_INTERVAL_MS` | 2,000 |
| Inbound rate limit | token bucket: 120 msgs/s sustained, burst 240, per session. Exceeded for 2 s → Disconnect(`RATE_LIMITED`) |
| Outbound reliable backlog | 256 KB per session → Disconnect(`KICKED`, "slow consumer") |
| Outbound unreliable queue | 32 messages per session; drop oldest when full |

### 5.9 Test vectors format

```json
{
  "version": 1,
  "vectors": [
    {
      "name": "hello_basic",
      "type": "Hello",
      "value": { "protocol_version": 1, "game_id": "poggersio", "client_build": "dev", "token": "" },
      "hex": "01 0100 0900 706f67676572 73696f ..."
    },
    { "name": "hello_bad_utf8", "type": "Hello", "hex": "...", "error": "InvalidUtf8" }
  ]
}
```

Each codec test does three things: decode `hex` and compare to `value`; encode `value` and compare to `hex`; for `error` vectors, check decode fails
with that error kind. The coordinator writes ≥ 3 vectors per message (typical, boundary lengths, malformed) at M0. Hex may contain
spaces (ignored).

---

## 6. Transport implementation notes (Agent S)

- **Crates:** `tokio`, `wtransport` (WebTransport over QUIC), `axum` (HTTP + WebSocket), `bytes`, `tracing`, `metrics` +
  `metrics-exporter-prometheus`, `rustls`. Pin exact versions at M0.
- **TLS:**
  - Dev: generate a self-signed ECDSA P-256 cert valid ≤ 14 days (browser requirement for `serverCertificateHashes`) at startup
    if `TW_DEV_CERT=1`. Serve its SHA-256 at `GET /dev/cert-hash` as JSON `{ "sha256": "<hex>" }` with permissive CORS **in dev only**.
  - Prod: `TW_CERT_PATH` / `TW_KEY_PATH` (e.g. Let's Encrypt). WebTransport needs UDP on the WT port reachable end to end.
    Most L7 proxies can't proxy it, so the game server terminates QUIC itself.
- **Session abstraction:** `Session { id, transport: Wt|Ws, outbound: SessionTx }`, where `SessionTx` has `send_reliable(Bytes)` and
  `send_unreliable(Bytes)` with the queue rules in §5.8. One tokio task per session for I/O. The game never awaits network I/O.
- **WebSocket hardening:** check `Origin` against `TW_ALLOWED_ORIGINS` (comma list; `*` in dev); max frame `MAX_MESSAGE`;
  per-IP connection cap `TW_MAX_CONN_PER_IP` (default 8).
- **Graceful shutdown:** on SIGTERM, send Disconnect(`SERVER_SHUTDOWN`) to all sessions, wait up to 2 s, exit.

---

## 7. Rooms runtime and the `Game` trait (Agent S)

### 7.1 Threading model
- A fixed pool of **room worker threads** (default = physical cores − 1). Each worker owns a set of rooms and runs a loop:
  sleep until the next tick deadline, then for each room: drain inbound messages → `tick()` → `snapshot()` per client → hand bytes to session queues.
- Rooms are assigned to the worker with the fewest players at creation. Rooms never move between workers.
- Inbound path: session task decodes the envelope header, then pushes `(client_id, msg_type, Bytes)` into the room's bounded MPSC queue (1,024 entries).
  Overflow → drop and count a metric. No locks on the hot path; game state is owned by the worker thread.

### 7.2 Tick timing
- Fixed `dt = 1 / tick_rate_hz`. If a worker falls behind by more than 3 ticks, it skips ahead (counts `tick_overrun_total`) instead of trying to catch up.
- `server_tick` is a per-room `u32` counter starting at 0.

### 7.3 `Game` trait (public API: keep it small, document every item)

```rust
pub trait Game: Send + 'static {
    /// Static config shared by all rooms of this game (loaded once).
    type Config: Send + Sync + 'static;

    fn game_id() -> &'static str;                       // matches Hello.game_id
    fn new(cfg: &Self::Config, ctx: &mut RoomCtx) -> Self;

    /// Validate join payload; return the game payload for RoomJoined, or a user-facing error.
    fn on_join(&mut self, ctx: &mut RoomCtx, client: ClientId, payload: &[u8]) -> Result<Vec<u8>, JoinError>;
    fn on_leave(&mut self, ctx: &mut RoomCtx, client: ClientId, reason: LeaveReason);

    /// Game messages (type 0x40–0x7F). Returning Err disconnects the client with PROTOCOL_ERROR.
    fn on_message(&mut self, ctx: &mut RoomCtx, client: ClientId, msg_type: u8, body: &[u8]) -> Result<(), ProtocolError>;

    /// Advance the simulation by one fixed step.
    fn tick(&mut self, ctx: &mut RoomCtx);

    /// Write this client's unreliable per-tick snapshot (full message incl. type byte), or nothing.
    fn snapshot(&mut self, ctx: &SnapshotCtx, client: ClientId, out: &mut Writer);

    /// Called after each tick; return true to keep the room alive even with zero humans.
    fn keep_alive(&self) -> bool { false }
}
```

`RoomCtx` provides: `room_id()`, `tick()`, `dt()`, `now()` (monotonic), `rng()` (seeded per room, `rand::rngs::SmallRng`),
`send_reliable(client, msg_type, |w| ...)`, `broadcast_reliable(msg_type, |w| ...)`, `kick(client, reason)`, `clients()`.

### 7.4 Matchmaking (single process, v1)
- `JoinRoom` with `room_kind`: QuickJoin picks the room of that kind with the **most humans below `room_capacity`**
  (fills rooms before opening new ones). If none, create one if `rooms < max_rooms`, else Reject(`SERVER_FULL`).
- Config: `TW_ROOM_CAPACITY` (default 100 humans), `TW_MAX_ROOMS` (default 64), `TW_EMPTY_ROOM_TTL_S` (default 60).

### 7.5 Room lifecycle
- A room with zero humans for `empty_room_ttl` and `keep_alive() == false` is dropped (its bots stop). **This is what fixes the
  "burns money while nobody plays" problem.**
- A server with zero rooms uses ~0% CPU.

### 7.6 Hooks for a future multi-box lobby
- `GET /status` → `{ "build", "uptime_s", "sessions", "rooms": [{ "id", "kind", "humans", "capacity" }], "load": 0.0-1.0 }`.
- `TW_PUBLIC_WT_URL`, `TW_PUBLIC_WS_URL` env vars, echoed in `/status`, so a lobby can redirect clients later.

### 7.7 Spatial grid (`tickwire_rooms::spatial::Grid<T: Copy>`)
- Uniform grid, configurable cell size (poggersio: 256 px). API: `clear()`, `insert(id, x, y, radius)`, `query_rect(min, max, |id| ...)`,
  `query_circle(x, y, r, |id| ...)`. Rebuilt every tick (simple and fast for ≤ 5,000 entities); no incremental updates in v1.

### 7.8 Metrics (`GET /metrics`, Prometheus text)
`tw_sessions{transport}`, `tw_rooms`, `tw_room_humans`, `tw_tick_duration_seconds` (histogram), `tw_tick_overrun_total`,
`tw_bytes_out_total{channel}`, `tw_bytes_in_total`, `tw_unreliable_dropped_total`, `tw_fragmented_total`, `tw_rate_limited_total`,
`tw_disconnects_total{reason}`.

---

## 8. TypeScript client SDK `@tickwire/client` (Agent C)

ESM, TypeScript strict, **zero runtime dependencies**, works in browsers only (Node is used only for tests).

```ts
export interface ConnectOptions {
  wtUrl?: string;                 // https://host:4433/tw
  wsUrl: string;                  // wss://host/tw/ws (required: fallback)
  gameId: string;
  clientBuild?: string;
  token?: string;
  transport?: "auto" | "webtransport" | "websocket";   // default "auto"
  wtTimeoutMs?: number;           // default 2500; auto falls back to WS after this
  certHashHex?: string;           // dev only: serverCertificateHashes
}

export async function connect(opts: ConnectOptions): Promise<Connection>;

export interface Connection {
  readonly transport: "webtransport" | "websocket";
  readonly sessionId: number;
  readonly tickRateHz: number;
  readonly rttMs: number;                       // EWMA from Ping/Pong
  serverTickNow(): number;                      // fractional estimated current server tick
  joinRoom(kind: string, payload: Uint8Array): Promise<{ roomId: number; payload: Reader }>;
  leaveRoom(): void;
  sendReliable(type: number, write: (w: Writer) => void): void;
  sendUnreliable(type: number, write: (w: Writer) => void): void;
  on(type: number, handler: (r: Reader) => void): () => void;   // game messages; returns unsubscribe
  onError(handler: (code: number, message: string) => void): () => void;
  onClose(handler: (reason: number, message: string) => void): () => void;
  close(): void;
}

export class Writer { u8; u16; u32; i16; i32; f32; varu32; str; bytes; finish(): Uint8Array }
export class Reader { u8; u16; u32; i16; i32; f32; varu32; str; bytes; remaining(): number }

/** Interpolation buffer for per-tick snapshots. */
export class SnapshotBuffer<S> {
  constructor(opts: { tickRateHz: number; delayTicks?: number /* default 2 */ ; capacity?: number /* default 32 */ });
  push(tick: number, snapshot: S): void;        // out-of-order and duplicate safe
  sample(serverTick: number): { a: S; b: S; t: number } | null;   // render at serverTick - delayTicks
}
```

Implementation notes:
- **WebTransport:** `new WebTransport(url, certHash ? { serverCertificateHashes: [{ algorithm: "sha-256", value }] } : undefined)`;
  open the control stream with `createBidirectionalStream()`; parse length-prefixed frames from a growable buffer; datagrams via
  `datagrams.readable` / `datagrams.writable`.
- **auto mode:** if `typeof WebTransport === "undefined"` or `wtUrl` is missing → WS immediately. Otherwise try WT; if no `Welcome` within
  `wtTimeoutMs`, abort it and connect WS. Remember a WT failure in `sessionStorage` for 10 minutes (wrapped in try/catch) to skip the wait.
- **Clock sync:** estimated server tick = `welcome.server_tick + (now − welcomeTime)/tickMs`, corrected from each `Pong.server_tick` with
  RTT/2 compensation, smoothed (don't let the estimate jump backwards).
- Never use `innerHTML` with server strings anywhere in the SDK or the games.

---

## 9. poggersio game server (Agent S)

### 9.1 Port strategy
`server/src/lib.rs` is the **rules reference**. Port it module by module into plain Rust structs owned by a `PoggersRoom: Game`:

| SpacetimeDB table | New in-memory structure |
|---|---|
| `Config` | `PoggersConfig` (loaded from defaults + optional `poggersio.toml`), also serialized into `RoomJoined` |
| `Player`, `Session` | `players: HashMap<PlayerId, Player>`, `client_to_player: HashMap<ClientId, PlayerId>` |
| `Cell` | `cells: Vec<Cell>` + `cell_index: HashMap<CellId, usize>` (or slotmap) |
| `Food`, `Virus`, `Ejected` | `Vec`s with IDs; food/virus changes accumulated in per-tick `added`/`removed` lists for deltas |
| `Emote` | `emotes: HashMap<String /*channel*/, [Option<String>; 8]>`, per room in v1 (sharing across rooms is a v2 decision) |
| `EmoteEvent`, `EmoteBurst` | short-lived `Vec`s; emitted as reliable events, expiry tracked for cooldowns only |
| `KillFeed`, `Leaderboard`, `ChannelStats` | computed state; sent as reliable messages |
| `TickSchedule` | replaced by the tickwire room loop |

Keep the **tick order** from `tick()`: move cells → move ejected → eat food → eat ejected → viruses → cell collisions → decay → bot AI →
top-up → expire effects → (every 20 ticks) leaderboard/channel stats. Use `Grid` for every "for each X, for each Y" loop.

IDs: `u32` everywhere on the wire (`PlayerId`, `CellId`, `FoodId`, ...), allocated per room, never reused within a room's lifetime
(wraps after 4B, which won't happen).

Player color: `hsl(hash(player_id) % 360, 70%, 55%)`, computed client-side from `player_id` (no wire field).

Bots: `bots_per_room` (default 12) with the same names, channels and AI. Bots don't count as humans for room lifecycle or capacity.

### 9.2 Viewport (interest management)
The server decides what each client sees:
- `total_mass` = sum of the player's cell masses (alive) or the last value (spectating after death).
- `view_scale = clamp((total_mass / 20)^0.2, 1.0, 4.0)`.
- View half-extents = `(1000 × view_scale, 600 × view_scale)` around the camera center (mass-weighted centroid of the player's cells,
  or the death position). Include entities whose circle intersects that rect expanded by 100 px.
- The snapshot carries the camera center and half-extents; the **client uses them for its camera and zoom**, so there's one source of truth.
- Hard cap: at most 255 cells and 255 ejected blobs per snapshot (nearest to the camera first). Typical snapshots should fit in one datagram.

### 9.3 Game messages (contract, goes into `protocol/SPEC.md` at M0)

Quantization: positions are `i16` in world pixels (map radius 6,000 fits ±32,767). Mass is `u16 mass_q = round(mass × 2)` (max 22,500 → 45,000).

**JoinRoom payload** (room_kind `""`): `str name` (1–16 after trim), `str channel` (0–24, lowercased by the server).

**RoomJoined payload:** `u32 your_player_id`, then `PoggersConfig`: `f32 map_radius`, `u16 tick_ms`, `u8 max_cells`, `f32 min_split_mass`,
`f32 eat_ratio`, `u8 emote_slots`, `f32 emote_unlock_mass`, `u16 emote_cooldown_ms`, `f32 burst_unlock_mass_eaten`, `f32 burst_min_mass`,
`u16 burst_cooldown_ms`, `f32 burst_radius`, `u16 emote_duration_ms`, `u16 merge_cooldown_ms`.
Followed immediately (reliable, same tick) by `PlayerInfo` for every player, `FoodFull`, `VirusSet`, every relevant `EmoteSet`,
`Leaderboard`, `ChannelStats`, `YouState`.

**Client → server:**

| ID | Name | Channel | Body | Rules |
|---|---|---|---|---|
| `0x40` | Input | U | `u32 seq`, `i16 target_x`, `i16 target_y` | Send at 20 Hz while alive and only when changed or every 250 ms. Server keeps the highest `seq`. |
| `0x41` | Split | R | empty | Same rules as the `split` reducer |
| `0x42` | Eject | R | `i16 dir_x`, `i16 dir_y` | Direction vector (any length > 0); server normalizes |
| `0x43` | ShowEmote | R | `u8 slot` | `show_emote` rules |
| `0x44` | BurstEmote | R | `u8 slot` | `burst_emote` rules |
| `0x45` | ImportEmotes | R | `u8 count` (≤64), then `count × (u8 slot, str url ≤256)` | Replaces the player's channel set; URLs must start with `https://` or be a bare name `[A-Za-z0-9_]{1,32}` |
| `0x46` | Respawn | R | empty | Only when dead; same name/channel as join |

Non-finite or out-of-range values → ignore the message (don't disconnect). Game errors go back as framework `Error` with codes:
`1001` name required, `1002` name too long, `1003` channel too long, `1004` need more mass to emote, `1005` emote on cooldown,
`1006` no emote in slot, `1007` burst locked, `1008` need more mass for burst, `1009` burst on cooldown, `1010` invalid emote URL,
`1011` not alive.

**Server → client:**

| ID | Name | Channel | Body |
|---|---|---|---|
| `0x80` | Snapshot | U | `u32 tick`, `u32 last_input_seq`, `i16 cam_x`, `i16 cam_y`, `u16 view_hw`, `u16 view_hh`, `u8 n_cells`, `n × (varu32 cell_id, varu32 player_id, i16 x, i16 y, u16 mass_q)`, `u8 n_ejected`, `n × (varu32 id, i16 x, i16 y)` |
| `0x81` | PlayerInfo | R | `varu32 player_id`, `str name`, `str channel`, `u8 flags` (bit0 = bot) |
| `0x82` | PlayerRemoved | R | `varu32 player_id` |
| `0x83` | FoodFull | R | `u16 n`, `n × (varu32 id, i16 x, i16 y, u8 color)` |
| `0x84` | FoodDelta | R | `u16 n_removed`, `n × varu32 id`, `u16 n_added`, `n × (varu32 id, i16 x, i16 y, u8 color)` (sent only on ticks with changes) |
| `0x85` | VirusSet | R | `u8 n`, `n × (varu32 id, i16 x, i16 y)` (full set, only when it changes) |
| `0x86` | YouState | R | `u8 alive`, `f32 total_mass_eaten`, `u32 emote_ready_tick`, `u32 burst_ready_tick` (sent on change) |
| `0x87` | Leaderboard | R | `u8 n` (≤10), `n × (varu32 player_id, u32 mass)` (1 Hz) |
| `0x88` | KillFeed | R | `str killer_name`, `str killer_channel`, `str victim_name`, `str victim_channel`, `u32 mass` |
| `0x89` | ChannelStats | R | `u8 n` (≤32, by total mass), `n × (str channel, u32 total_mass, u16 players, u16 kills)` (1 Hz) |
| `0x8A` | EmoteSet | R | `str channel`, `u8 n`, `n × (u8 slot, str url)` (on join for all channels present, and on change) |
| `0x8B` | EmoteFx | R | `varu32 player_id`, `varu32 cell_id`, `u8 slot`, `u8 kind` (0 = show, 1 = burst), `i16 x`, `i16 y`, `u16 duration_ms` |
| `0x8C` | Died | R | `str killer_name` (empty if none), `u32 final_mass`, `u32 alive_ms` |

Food and virus messages are **room-wide** (not viewport-filtered): ~1,200 foods, ~30 viruses, a few changes per tick. If profiling shows
they matter, viewport-filter them in v2.

### 9.4 Bugs in the reference to fix during the port (each needs a unit test)
1. **Food/virus spawn radius:** `random_map_point` uses `gen_range(0.0..map_radius.sqrt())` → everything spawns within ~77 px of the center.
   Correct uniform-disc sampling: `dist = map_radius * sqrt(rng.gen::<f32>())`.
2. **Emote burst does nothing:** `burst_emote` charges mass but never applies `emote_burst_damage` / `emote_burst_knockback`. Implement: on burst,
   every enemy cell within `burst_radius` loses `damage × mass` (min mass 10; mass removed is not given to anyone) and gets a boost of `knockback`
   away from the burst center.
3. **Leaderboard cadence:** uses `timestamp / tick_ms % 20 == 0`, which can skip with scheduler jitter. Use the room tick counter: `tick % 20 == 0`.
4. **Stale masses in loops:** `eat_food`, `eat_ejected`, `virus_collisions` iterate over a snapshot of cells taken before the loop, so radius and mass
   go stale within a tick. Re-read the current cell (or update in place) for each check.
5. **Channel kill counts** only count the last 50 kill-feed rows. Track `kills` per channel as a counter in room state.
6. **No input validation:** `set_target` / `eject_mass` accept NaN/∞. Ignore non-finite input.
7. **Room never sleeps:** handled by §7.5 (rooms with no humans close).

### 9.5 Server binary
`poggersio-server` (in `games/poggersio/server`) builds a tickwire `ServerBuilder` with `PoggersRoom`, reads env (§13.2), and serves the client's
built static files from `TW_STATIC_DIR` if set (convenient for single-box deploys).

---

## 10. poggersio browser client (Agent C)

### 10.1 Stack
Vite + TypeScript (strict), no UI framework, Canvas 2D. Depends only on `@tickwire/client` (workspace).
Dev: `npm run dev -w games/poggersio/client`, reading `VITE_WT_URL`, `VITE_WS_URL`, `VITE_DEV_CERT_HASH_URL` (fetches `/dev/cert-hash`).

### 10.2 Screens
1. **Login:** name (1–16), channel (0–24), Play button, transport indicator after connecting. Remember the last name/channel in `localStorage` (try/catch).
2. **Game:** canvas full-window plus HUD.
3. **Death:** killer, final mass, time alive, "Play again" → `Respawn`.
4. **Disconnected:** reason text, "Reconnect" → back to Login with fields prefilled.

### 10.3 Rendering
- Camera: snapshot `cam_x/cam_y` and `view_hw/view_hh` interpolated through `SnapshotBuffer`. Zoom = `min(canvasW / (2·view_hw), canvasH / (2·view_hh))`.
- Draw order: map disc (radius from config, subtle grid, dark red outside) → food (pre-rendered to an offscreen canvas, updated on FoodFull/FoodDelta) →
  ejected → viruses (spiky green, radius `sqrt(100)·4`) → cells (radius `sqrt(mass)·4`, sorted by mass ascending, fill = player color,
  name + mass label when radius on screen > 14 px) → emote FX → HUD.
- Interpolate cells by `cell_id` between the two sampled snapshots. A cell present only in the newer snapshot appears at its position; a cell missing from the newer one
  disappears (no extrapolation).

### 10.4 Input
- Mouse → world coordinates → `Input` at 20 Hz (only when changed, or every 250 ms as a keepalive).
- Space or left-click: `Split`. W or right-click: `Eject` toward the cursor (hold W = repeat every 100 ms).
- Keys 1–8: `ShowEmote(slot)`. Shift+1–8: `BurstEmote(slot)`. Show cooldowns from `YouState` ticks vs `serverTickNow()`.
- Emote import panel (button in HUD): textarea, one entry per line (`url` or `slot:url`), sends `ImportEmotes`.

### 10.5 HUD
Leaderboard (top 10, highlight self), kill feed (last 8, fade after 6 s), your mass, channel stats (top 5), emote slot bar with cooldown/locked
states, burst unlock progress (`total_mass_eaten / burst_unlock_mass_eaten`), error toasts from `onError`, F3 perf overlay (fps, RTT, transport,
snapshot bytes/s, buffered snapshots).

### 10.6 FakeServer (unblocks M4 before the real server exists)
`games/poggersio/client/src/dev/FakeServer.ts` implements the same `Connection` interface in-process: it simulates a few dozen moving cells,
food and viruses with simple rules and emits correctly **encoded** messages (round-tripping through the codec catches encoding bugs).
Enabled with `?fake=1`. Kept after M4 for UI work and screenshots.

### 10.7 Emote images
`EmoteSet` URLs: `https://...` loaded via `new Image()` with `crossOrigin = "anonymous"` and cached per URL; bare names → a built-in emoji fallback map
(e.g. `PogChamp → 😮`, `KEKW → 😂`, default `💬`). Show FX for `duration_ms`, fading out over the last 30%. Burst FX = expanding ring at
`burst_radius` plus the emote.

### 10.8 Acceptance checklist (S4 gate)
- [ ] Login → join → spawn among bots, on WebTransport (Chromium) and with `?transport=websocket`
- [ ] Steering is smooth at 60 fps (interpolated, no 20 fps jumps)
- [ ] Split and eject work; 16-cell cap respected; merge after cooldown
- [ ] Food is spread across the whole map (bug 9.4.1 fixed); eating food grows you
- [ ] Eating a bot's last cell kills it; kill feed and leaderboard update
- [ ] Virus contact splits you when big
- [ ] Emote import works; 1–8 show emotes; burst unlocks and visibly damages and knocks back nearby enemies
- [ ] Leaderboard, kill feed and channel stats update live
- [ ] Death → Play again works; server restart → Disconnected screen → Reconnect works
- [ ] No console errors; F3 overlay shows sane numbers

---

## 11. Performance targets (S5 gate)

Measured with `tools/loadtest` on one 8-core x86 box, bots spread over rooms of 100 humans + 12 AI bots:

| Metric | Target |
|---|---|
| Concurrent bot clients | 2,000 sustained for 10 minutes (stretch: 4,000) |
| `tw_tick_duration_seconds` p99 per room | < 4 ms |
| Tick overruns | 0 |
| Server CPU | < 60% total at 2,000 clients |
| Downstream per client | < 15 KB/s average |
| Snapshot size p95 | ≤ 1,100 bytes (fits one datagram) |
| Memory | < 2 GB RSS at 2,000 clients |

**Load-test bots** (`tools/loadtest`): connect via WT (using `wtransport` client mode with the dev cert hash) or WS (`--transport ws|wt|mix`),
join, wander with random targets, split/eject occasionally, decode every snapshot (to catch encoder bugs), and report connect success rate,
p50/p99 snapshot inter-arrival, bytes/s, and disconnect reasons. Flags: `--clients N --ramp-per-sec R --duration 600s --url ...`.

---

## 12. CI (`.github/workflows/ci.yml`, owned by the coordinator)

Jobs:
1. **rust:** `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace` (includes vector tests).
2. **ts:** `npm ci`, `npm run typecheck --workspaces`, `npm test --workspaces` (Vitest; includes vector tests).
3. **contract:** a script asserts that the Rust and TS constants match `protocol/constants.json`, and greps `crates/` and `packages/` for game words
   (`poggers`, `cell`, `food`, `virus`, `emote`), failing on any match outside comments in test fixtures.
4. **e2e** (from S2 on): build the echo/dots/poggersio servers, start them with `TW_DEV_CERT=1`, run Playwright Chromium tests for WT, WS and auto.

Agents run jobs 1–3 locally before every push.

---

## 13. Deployment (written up in `docs/DEPLOY.md` at M6)

### 13.1 Target topology for 10k concurrent players
- 2–3 dedicated boxes (8 cores, 32–64 GB RAM, **1 Gbps unmetered** bandwidth, e.g. Hetzner/OVH), each running `poggersio-server`.
- Each box: UDP 443 (WebTransport) and TCP 443 (WebSocket + static client) open; a real TLS cert for the box's hostname.
- One small VM for Postgres (v2 persistence) and a future lobby.
- Avoid hosts that bill per GB of egress: at ~1 Gbps peak, egress dominates cost.

### 13.2 Environment variables

| Var | Default | Meaning |
|---|---|---|
| `TW_WT_BIND` | `0.0.0.0:4433` | WebTransport UDP bind |
| `TW_HTTP_BIND` | `0.0.0.0:8080` | HTTP + WebSocket bind |
| `TW_CERT_PATH` / `TW_KEY_PATH` | — | PEM cert/key (prod) |
| `TW_DEV_CERT` | `0` | `1` = generate a self-signed cert and expose `/dev/cert-hash` |
| `TW_ALLOWED_ORIGINS` | `*` (dev) | WS Origin allow-list |
| `TW_ROOM_CAPACITY` | `100` | Humans per room |
| `TW_MAX_ROOMS` | `64` | Rooms per process |
| `TW_EMPTY_ROOM_TTL_S` | `60` | Close rooms with no humans after this |
| `TW_WORKERS` | cores − 1 | Room worker threads |
| `TW_MAX_CONN_PER_IP` | `8` | Per-IP connection cap |
| `TW_STATIC_DIR` | — | Serve built client files |
| `TW_PUBLIC_WT_URL` / `TW_PUBLIC_WS_URL` | — | Advertised in `/status` |
| `RUST_LOG` | `info` | tracing filter |
| `POGGERS_BOTS_PER_ROOM` | `12` | AI bots per room |

---

## 14. Definition of done (per PR)

- Owns only files in the agent's area (§2.1).
- Fast checks green locally (§12 jobs 1–3).
- New logic has tests; protocol code passes vectors.
- Public Rust items and exported TS symbols have doc comments.
- `COORDINATION.md` status updated.
- No game concepts in `crates/` or `packages/`.

---

## Appendix A: Kickoff prompt for Agent S (server)

> You are **Agent S**, the server engineer for the tickwire + poggersio rebuild in this repo. Read `TICKWIRE_HANDOFF.md` fully, then
> `protocol/SPEC.md`, `protocol/constants.json`, `COORDINATION.md`, and the reference game rules in `server/src/lib.rs`.
>
> You own `crates/**`, `games/poggersio/server/**`, `examples/dots/server/**`, `tools/loadtest/**`. Do not edit anything else. If the protocol
> needs to change, add a row to "Contract change requests" in `COORDINATION.md` and continue with other work until the coordinator answers.
>
> Work milestone by milestone (M1 → M5), Agent S column only. Use branches `tw/server/<milestone>-<topic>`, small PRs into `main`, CI green
> before asking for review. Update your section of `COORDINATION.md` at the end of every work session. Merge `main` at every sync point.
> Start with M1: `tickwire-proto` passing `protocol/vectors/framework.json`, then the WebSocket transport and the echo example
> (Agent C needs it for the S1 gate, so ship it early).

## Appendix B: Kickoff prompt for Agent C (client)

> You are **Agent C**, the client engineer for the tickwire + poggersio rebuild in this repo. Read `TICKWIRE_HANDOFF.md` fully, then
> `protocol/SPEC.md`, `protocol/constants.json`, and `COORDINATION.md`.
>
> You own `packages/**`, `games/poggersio/client/**`, `examples/dots/client/**`. Do not edit anything else. If the protocol needs to change,
> add a row to "Contract change requests" in `COORDINATION.md` and continue with other work until the coordinator answers.
>
> Work milestone by milestone (M1 → M5), Agent C column only. Use branches `tw/client/<milestone>-<topic>`, small PRs into `main`, CI green
> before asking for review. Update your section of `COORDINATION.md` at the end of every work session. Merge `main` at every sync point.
> Never block on the server: build against the vectors and, in M4, against `FakeServer`. Start with M1: the `@tickwire/client` codec passing
> `protocol/vectors/framework.json`, then the WebSocket transport.

## Appendix C: Coordinator playbook

1. **M0:** do everything in §3 M0. Write the vectors carefully: they are the contract both agents test against.
2. Launch Agent S and Agent C with Appendix A/B as their first message (via `create_session`, or your orchestrator's equivalent).
3. Subscribe to every PR they open. Review against §14. Merge when green.
4. Answer contract change requests within one work cycle. Every approved change: update `protocol/`, bump the version if
   wire-incompatible, add vectors, merge, `send_message` both agents.
5. At each sync point: merge both sides, run the gate test yourself, record pass/fail in the decisions log, then tell both agents
   to start the next milestone. If a gate fails, file the failure in the owning agent's section as a blocker.
6. Watch for drift: scope creep into another agent's area, game words in `crates/`/`packages/`, or work skipping ahead past an unpassed gate.
7. **M6:** deploy doc, delete `server/` and `CLIENT_HANDOFF.md`, update README, tag `tickwire-v0.1.0`.
