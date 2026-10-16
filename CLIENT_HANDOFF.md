# poggersio Frontend — Handoff for Claude

## Mission

Build the browser client for **poggersio**, a Twitch-emote themed agar.io-style game. The entire game server already exists and runs: a SpacetimeDB 2.12 module in Rust at `server/src/lib.rs` (~1,176 lines, compiles clean, published locally). There is **no frontend yet** — you are building it from scratch.

The game: you are a cell in a circular arena. Eat food and smaller players, avoid bigger ones, split/eject mass, and use Twitch emotes — `show_emote` as a flex, `burst_emote` as an unlockable AoE attack. 12 AI bots with Twitch-culture names (Kappa, KEKW, monkaS, EZ Clap…) already play the game.

## Current state (verified working)

- Local SpacetimeDB server running: `spacetime start --in-memory` (listens on `127.0.0.1:3000`, nickname `local`)
- Module published: `spacetime publish --server local poggersio` (database identity `c2007a1caf673446ca62cd617111856f8b4d3dd2cf42269a9a4dd3bbf87e949d`)
- Tick loop confirmed running (bots move, leaderboard updates every 1s)
- CLI smoke test of `join` works: `spacetime call --server local poggersio join '"Jared", "xqc"'`
- **Known server gap**: `burst_emote` creates the `EmoteBurst` row and charges mass, but no tick code applies the AoE damage/knockback yet (`emote_burst_damage`, `emote_burst_knockback` in Config are unused). Treat burst as visual-only for now, or fix the server (add a `apply_emote_bursts` step in `tick()` in `server/src/lib.rs`).

## Repo layout

```
poggersio/
├── README.md          # placeholder
├── server/            # SpacetimeDB module (Rust) — DO NOT restructure
│   ├── Cargo.toml     # spacetimedb = "2.12.0", cdylib, edition 2024
│   └── src/lib.rs
└── client/            # ← CREATE THIS (suggested: Vite + TypeScript, no framework needed)
```

## Step 1 — Generate typed client bindings

The SpacetimeDB CLI (installed at `~/.local/bin/spacetime`, v2.12.0) generates TypeScript bindings from the module:

```bash
cd server
spacetime generate --lang typescript --module-path . --out-dir ../client/src/module_bindings
```

This produces `index.ts` (exports `DbConnection`, `DbConnectionBuilder`, `SubscriptionBuilder`, `tables`, `reducers`, and all row types), plus per-table/per-reducer files. Field names are camelCased (`totalMassEaten`, `targetX`, `expiresAt`). Verified working — do not hand-write these.

## Step 2 — Scaffold the client

Suggested stack (keep it simple, canvas 2D is enough):

- **Vite + TypeScript**, no UI framework (or React if you prefer — your call)
- `npm i spacetimedb` — the SDK package the generated code imports from (`import ... from "spacetimedb"`)
- Canvas 2D renderer with camera following the player's biggest cell, zoom scaled to cell size
- Game loop: `requestAnimationFrame` render at 60fps; server state arrives via subscriptions, interpolate positions between ticks (server ticks at 50ms)

## Step 3 — Connect & subscribe

```ts
import { DbConnectionBuilder, tables, reducers } from "./module_bindings";

const conn = DbConnectionBuilder.builder()
  .withUri("ws://127.0.0.1:3000")
  .withModuleName("poggersio")
  .build();

// Subscribe to everything the HUD needs
conn.subscriptionBuilder()
  .onApplied(() => { /* hide loading screen */ })
  .subscribe([
    "SELECT * FROM player",
    "SELECT * FROM cell",
    "SELECT * FROM food",
    "SELECT * FROM virus",
    "SELECT * FROM ejected",
    "SELECT * FROM emote",
    "SELECT * FROM emote_event",
    "SELECT * FROM emote_burst",
    "SELECT * FROM kill_feed",
    "SELECT * FROM leaderboard",
    "SELECT * FROM channel_stats",
    "SELECT * FROM config",
  ]);
```

(Exact builder API may differ slightly per SDK version — the generated `index.ts` is the source of truth. `DbConnection` exposes `conn.db.<table>` and `conn.reducers.<reducer>`.)

## Step 4 — Game flow

1. **Login screen**: name (max 16 chars) + channel (max 24 chars, lowercased server-side). Call `conn.reducers.join(name, channel)`.
2. **On join success**: your `Player` row appears (match by `conn.identity`); your starting `Cell` has mass 20.
3. **Controls**:
   - Mouse move → `conn.reducers.setTarget(worldX, worldY)` (throttle to ~10-20/s; server clamps to map disc radius 6000)
   - Space / click → `conn.reducers.split()`
   - W / right-click → `conn.reducers.ejectMass(dx, dy)` (direction vector toward cursor)
   - Keys 1–8 → `conn.reducers.showEmote(slot)` (slots 0–7)
   - Shift + 1–8 → `conn.reducers.burstEmote(slot)`
   - Optional: emote import UI → `conn.reducers.importEmotes({ name, emotes })` where `emotes: string[]`, entries `"url"` or `"slot:url"` e.g. `"0:https://example.com/pog.png"`
4. **Death**: your `Player.alive` flips false / cells vanish → death screen with "Play again" (call `join` again — server allows rejoin after death; disconnect also kills you).

## Step 5 — Rendering details

World → screen: camera centered on your biggest cell; zoom = `baseZoom / sqrt(yourTotalMass)` clamped sensibly. Draw order:

1. Map disc (radius 6000, center origin) — dark background, subtle grid, red border outside
2. Food (1,200 dots, use `color` u8 as hue seed)
3. Ejected mass blobs (12-mass pellets)
4. Viruses (spiky green circles, mass 100 → radius 40)
5. Cells: circle radius `sqrt(mass) * 4`, fill by player, name + mass label, outline
6. Emote overlays: `emote_event` / `emote_burst` rows have `x, y, emoteId, expiresAt` — draw the emote image (from `emote.url` via `emoteId`) above the cell until expiry (2.5s), fade out
7. HUD: leaderboard (top 10, `leaderboard` table), kill feed (last ~8 of `kill_feed`), your mass, channel stats panel

Emote images: `emote.url` is a URL or name — client resolves rendering (remote URLs via `<img>` cache; unknown names → fallback emoji). Bots have no imported emotes, so mostly human channels show them.

## Server API reference (all verified from source)

**Reducers** (call via `conn.reducers.*`, all return errors as strings — surface them as toasts):

| Reducer | Args | Notes |
|---|---|---|
| `join` | `name: string, channel: string` | Errors: "already joined", "name required", "name too long (max 16)", "channel too long (max 24)" |
| `leave` | — | Kills you |
| `setTarget` | `x: f32, y: f32` | World coords, clamped to map disc |
| `split` | — | Silently no-ops if at 16-cell cap or no cell ≥ 36 mass |
| `ejectMass` | `dx: f32, dy: f32` | Direction vector (normalized server-side); errors "direction required" |
| `importEmotes` | `pack: { name: string, emotes: string[] }` | Replaces channel's set; max 64 entries, slots 0–7 |
| `showEmote` | `slot: u32` | Needs emote in slot, biggest cell ≥ 50 mass, 1.5s cooldown |
| `burstEmote` | `slot: u32` | Needs 2,500 lifetime mass eaten, biggest cell ≥ 200 mass, 8s cooldown, costs 10% of biggest cell |

**Tables** (subscribe; camelCase fields):

- `Player`: `id, identity, name, channel, isBot, totalMassEaten, alive, joinedAt`
- `Cell`: `id, playerId, x, y, mass, targetX, targetY, boost, boostDx, boostDy, mergeAt`
- `Food`: `id, x, y, mass, color`
- `Virus`: `id, x, y, mass`
- `Ejected`: `id, x, y, mass, boost, boostDx, boostDy, ownerId`
- `Emote`: `id, channel, slot, url`
- `EmoteEvent`: `id, playerId, cellId, emoteId, x, y, expiresAt`
- `EmoteBurst`: `id, playerId, cellId, emoteId, x, y, expiresAt`
- `KillFeed`: `id, killerName, killerChannel, victimName, victimChannel, mass, at`
- `Leaderboard`: `rank, playerId, name, channel, mass`
- `ChannelStats`: `channel, totalMass, playerCount, kills`
- `Config`: singleton `id=0` with all tuning constants (`tickMs=50`, `mapRadius=6000`, `eatRatio=1.25`, etc.) — read limits from here, don't hardcode

**Key formulas** (for prediction/interpolation):
- Speed: `780 / mass^0.44`, min 60 px/s
- Radius: `sqrt(mass) * 4`
- Decay: cells > 100 mass shrink 0.2%/s
- Eat rule: eater ≥ 1.25× victim mass AND covers victim center

## Dev workflow

```bash
# Terminal 1 (if server not already running)
spacetime start --in-memory

# Terminal 2 — publish/re-publish after any server change
cd server && spacetime publish --server local poggersio

# Terminal 3 — client dev server
cd client && npm run dev   # Vite on http://localhost:5173
```

Note: `--in-memory` wipes state on restart; re-publish re-runs `init` (food/viruses/bots respawn). For persistent local dev, drop `--in-memory`.

## Acceptance checklist

- [ ] Login screen → join → spawn visible among bots
- [ ] Mouse steering works; cell moves smoothly (interpolated, not 20fps jumpy)
- [ ] Split (Space) and eject (W) work; 16-cell cap respected visually
- [ ] Eating food grows you; eating a bot's last cell kills it and kill feed updates
- [ ] Virus contact splits you when big
- [ ] Emote import UI works; keys 1–8 show emotes over your cell; burst fires when unlocked
- [ ] Leaderboard + kill feed + channel stats render live
- [ ] Death screen → rejoin works
- [ ] No console errors; reconnects gracefully after server restart

## Out of scope (don't build)

- Real Twitch API integration (emote import is simulated by design)
- Mobile/touch controls
- Server changes beyond the optional burst-damage fix noted above