// poggersio — Twitch-emote agar.io-style game server
// SpacetimeDB 2.12 module
use std::time::Duration;

use spacetimedb::rand::Rng;
use spacetimedb::{
    reducer, table, Identity, ReducerContext, ScheduleAt, SpacetimeType, Table, Timestamp,
};

// ---------------------------------------------------------------------------
// Tuning constants
// ---------------------------------------------------------------------------

const TICK_MS: u64 = 50;
const MAP_RADIUS: f32 = 6000.0;
const SPAWN_RADIUS: f32 = 400.0;
const FOOD_COUNT: u32 = 1200;
const FOOD_MASS: f32 = 1.0;
const VIRUS_COUNT: u32 = 30;
const VIRUS_MASS: f32 = 100.0;
const VIRUS_SPLIT_MASS: f32 = 35.0; // mass gained by a cell hit by a virus
const BOT_COUNT: u32 = 12;
const MAX_CELLS_PER_PLAYER: u32 = 16;
const MIN_SPLIT_MASS: f32 = 36.0;
const SPLIT_BOOST: f32 = 780.0; // initial split impulse px/s
const EJECT_MASS: f32 = 12.0;
const EJECT_MIN_MASS: f32 = 35.0;
const EJECT_SPEED: f32 = 800.0;
const EJECT_LOSS: f32 = 14.0; // mass lost per ejection
const MAX_MASS: f32 = 22500.0;
const DECAY_RATE: f32 = 0.002; // fraction of mass lost per second
const MERGE_COOLDOWN_MS: u64 = 30_000;
const EAT_RATIO: f32 = 1.25; // eater must be 25% bigger
const BASE_SPEED: f32 = 780.0;
const SPEED_EXP: f32 = 0.44;
const MIN_SPEED: f32 = 60.0;
const MAX_NAME_LEN: usize = 16;
const MAX_CHANNEL_LEN: usize = 24;
const EMOTE_SLOTS: u32 = 8;
const EMOTE_UNLOCK_MASS: f32 = 50.0;
const EMOTE_COOLDOWN_MS: u64 = 1500;
const EMOTE_DURATION_MS: u64 = 2500;
const EMOTE_BURST_UNLOCK_MASS_EATEN: f32 = 2500.0;
const EMOTE_BURST_MIN_MASS: f32 = 200.0;
const EMOTE_BURST_COOLDOWN_MS: u64 = 8000;
const EMOTE_BURST_RADIUS: f32 = 420.0;
const EMOTE_BURST_DAMAGE: f32 = 0.35; // fraction of victim mass removed
const EMOTE_BURST_KNOCKBACK: f32 = 900.0;
const EMOTE_BURST_MASS_COST: f32 = 0.10; // fraction of own mass spent
const LEADERBOARD_SIZE: usize = 10;
const LEADERBOARD_INTERVAL_TICKS: u64 = 20; // recompute every 1s
const MAX_EMOTES_PER_CHANNEL: u32 = 64;
const KILL_FEED_MAX: usize = 50;

const BOT_NAMES: [&str; 16] = [
    "PogChamp", "Kappa", "KEKW", "OMEGALUL", "PepeLaugh", "monkaS", "WideHardo", "Sadge",
    "Copium", "CatJAM", "Pepega", "5Head", "HYPERS", "EZ Clap", "BibleThump", "Pog",
];
const BOT_CHANNELS: [&str; 8] = [
    "xqc", "kai cenat", "shroud", "ninja", "pokelawls", "nickmercs", "sodapoppin", "forsen",
];

// ---------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------

/// Global singleton config (id = 0). Written once by `init`.
#[table(accessor = config, public)]
struct Config {
    #[primary_key]
    id: u32,
    tick_ms: u64,
    map_radius: f32,
    food_count: u32,
    virus_count: u32,
    bot_count: u32,
    max_cells_per_player: u32,
    min_split_mass: f32,
    max_mass: f32,
    merge_cooldown_ms: u64,
    eat_ratio: f32,
    emote_slots: u32,
    emote_unlock_mass: f32,
    emote_cooldown_ms: u64,
    emote_burst_unlock_mass_eaten: f32,
    emote_burst_min_mass: f32,
    emote_burst_cooldown_ms: u64,
    emote_burst_radius: f32,
    emote_burst_damage: f32,
    emote_burst_knockback: f32,
    emote_burst_mass_cost: f32,
    max_name_len: u32,
    max_channel_len: u32,
}

#[table(accessor = player, public)]
struct Player {
    #[primary_key]
    #[auto_inc]
    id: u64,
    identity: Identity,
    name: String,
    channel: String,
    is_bot: bool,
    /// Total mass ever eaten — gates the emote-burst unlock.
    total_mass_eaten: f32,
    /// False once the player has lost all cells.
    alive: bool,
    joined_at: Timestamp,
}

#[table(accessor = session, public)]
struct Session {
    #[primary_key]
    #[auto_inc]
    id: u64,
    #[unique]
    identity: Identity,
    player_id: u64,
    connected: bool,
}

#[derive(Clone)]
#[table(accessor = cell, public)]
struct Cell {
    #[primary_key]
    #[auto_inc]
    id: u64,
    #[index(btree)]
    player_id: u64,
    x: f32,
    y: f32,
    mass: f32,
    /// Movement target in world space; the tick reducer steers toward it.
    target_x: f32,
    target_y: f32,
    /// Remaining split impulse (px/s), decays each tick.
    boost: f32,
    boost_dx: f32,
    boost_dy: f32,
    /// Earliest time this cell may merge with siblings.
    merge_at: Timestamp,
}

#[table(accessor = food, public)]
struct Food {
    #[primary_key]
    #[auto_inc]
    id: u64,
    x: f32,
    y: f32,
    mass: f32,
    /// Cosmetic color seed for the client.
    color: u8,
}

#[table(accessor = virus, public)]
struct Virus {
    #[primary_key]
    #[auto_inc]
    id: u64,
    x: f32,
    y: f32,
    mass: f32,
}

#[table(accessor = ejected, public)]
struct Ejected {
    #[primary_key]
    #[auto_inc]
    id: u64,
    x: f32,
    y: f32,
    mass: f32,
    /// Remaining eject impulse (px/s), decays each tick.
    boost: f32,
    boost_dx: f32,
    boost_dy: f32,
    owner_id: u64,
}

#[table(accessor = emote, public)]
struct Emote {
    #[primary_key]
    #[auto_inc]
    id: u64,
    #[index(btree)]
    channel: String,
    /// Slot index 0..EMOTE_SLOTS.
    slot: u32,
    /// URL or name of the emote image; client resolves rendering.
    url: String,
}

#[table(accessor = emote_event, public)]
struct EmoteEvent {
    #[primary_key]
    #[auto_inc]
    id: u64,
    #[index(btree)]
    player_id: u64,
    cell_id: u64,
    emote_id: u64,
    x: f32,
    y: f32,
    expires_at: Timestamp,
}

#[table(accessor = emote_burst, public)]
struct EmoteBurst {
    #[primary_key]
    #[auto_inc]
    id: u64,
    #[index(btree)]
    player_id: u64,
    cell_id: u64,
    emote_id: u64,
    x: f32,
    y: f32,
    expires_at: Timestamp,
}

#[table(accessor = kill_feed, public)]
struct KillFeed {
    #[primary_key]
    #[auto_inc]
    id: u64,
    killer_name: String,
    killer_channel: String,
    victim_name: String,
    victim_channel: String,
    mass: f32,
    at: Timestamp,
}

#[table(accessor = leaderboard, public)]
struct Leaderboard {
    #[primary_key]
    rank: u32,
    player_id: u64,
    name: String,
    channel: String,
    mass: f32,
}

#[table(accessor = channel_stats, public)]
struct ChannelStats {
    #[primary_key]
    channel: String,
    total_mass: f32,
    player_count: u32,
    kills: u32,
}

/// Drives the fixed-step game loop.
#[table(accessor = tick_schedule, scheduled(tick))]
struct TickSchedule {
    #[primary_key]
    #[auto_inc]
    scheduled_id: u64,
    scheduled_at: ScheduleAt,
}

// ---------------------------------------------------------------------------
// Reducer argument types
// ---------------------------------------------------------------------------

#[derive(SpacetimeType)]
pub struct EmotePack {
    pub name: String,
    /// Entries are "url" or "slot:url", e.g. "0:https://.../pog.png".
    pub emotes: Vec<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn get_config(ctx: &ReducerContext) -> Config {
    ctx.db.config().id().find(0u32).expect("config row missing")
}

fn spawn_point(ctx: &ReducerContext) -> (f32, f32) {
    let mut rng = ctx.rng();
    let angle: f32 = rng.gen_range(0.0..std::f32::consts::TAU);
    let dist: f32 = rng.gen_range(0.0..SPAWN_RADIUS);
    (angle.cos() * dist, angle.sin() * dist)
}

fn random_map_point(ctx: &ReducerContext, cfg: &Config) -> (f32, f32) {
    let mut rng = ctx.rng();
    let angle: f32 = rng.gen_range(0.0..std::f32::consts::TAU);
    let dist: f32 = rng.gen_range(0.0..cfg.map_radius.sqrt());
    (angle.cos() * dist, angle.sin() * dist)
}

fn cell_speed(mass: f32) -> f32 {
    (BASE_SPEED / mass.powf(SPEED_EXP)).max(MIN_SPEED)
}

fn cell_radius(mass: f32) -> f32 {
    mass.sqrt() * 4.0
}

fn now_plus_ms(ctx: &ReducerContext, ms: u64) -> Timestamp {
    ctx.timestamp + Duration::from_millis(ms)
}

/// Insert a player + starting cell. Shared by `join` and bot spawning.
fn create_player(ctx: &ReducerContext, identity: Identity, name: String, channel: String, is_bot: bool) -> u64 {
    let (x, y) = spawn_point(ctx);
    let player = ctx.db.player().insert(Player {
        id: 0,
        identity,
        name: name.clone(),
        channel: channel.clone(),
        is_bot,
        total_mass_eaten: 0.0,
        alive: true,
        joined_at: ctx.timestamp,
    });
    ctx.db.cell().insert(Cell {
        id: 0,
        player_id: player.id,
        x,
        y,
        mass: 20.0,
        target_x: x,
        target_y: y,
        boost: 0.0,
        boost_dx: 0.0,
        boost_dy: 0.0,
        merge_at: ctx.timestamp,
    });
    player.id
}

fn spawn_bot(ctx: &ReducerContext) {
    let mut rng = ctx.rng();
    let name_idx: usize = rng.gen_range(0..BOT_NAMES.len());
    let chan_idx: usize = rng.gen_range(0..BOT_CHANNELS.len());
    create_player(
        ctx,
        Identity::ZERO,
        BOT_NAMES[name_idx].to_string(),
        BOT_CHANNELS[chan_idx].to_string(),
        true,
    );
}

fn alive_bot_count(ctx: &ReducerContext) -> u32 {
    ctx.db
        .player()
        .iter()
        .filter(|p| p.is_bot && p.alive)
        .count() as u32
}

fn player_cells(ctx: &ReducerContext, player_id: u64) -> Vec<Cell> {
    ctx.db.cell().player_id().filter(player_id).collect()
}

fn dist2(ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let dx = ax - bx;
    let dy = ay - by;
    dx * dx + dy * dy
}

fn record_kill(ctx: &ReducerContext, killer_id: u64, victim_id: u64, mass: f32) {
    let (Some(killer), Some(victim)) = (
        ctx.db.player().id().find(killer_id),
        ctx.db.player().id().find(victim_id),
    ) else {
        return;
    };
    ctx.db.kill_feed().insert(KillFeed {
        id: 0,
        killer_name: killer.name.clone(),
        killer_channel: killer.channel.clone(),
        victim_name: victim.name.clone(),
        victim_channel: victim.channel.clone(),
        mass,
        at: ctx.timestamp,
    });
}

/// Remove all of a player's cells and mark them dead.
fn kill_player(ctx: &ReducerContext, victim_id: u64, killer_id: Option<u64>, mass: f32) {
    for c in player_cells(ctx, victim_id) {
        ctx.db.cell().id().delete(c.id);
    }
    if let Some(mut p) = ctx.db.player().id().find(victim_id) {
        p.alive = false;
        ctx.db.player().id().update(p);
    }
    if let Some(kid) = killer_id {
        record_kill(ctx, kid, victim_id, mass);
    }
}

// ---------------------------------------------------------------------------
// Lifecycle reducers
// ---------------------------------------------------------------------------

#[reducer(init)]
fn init(ctx: &ReducerContext) {
    ctx.db.config().insert(Config {
        id: 0,
        tick_ms: TICK_MS,
        map_radius: MAP_RADIUS,
        food_count: FOOD_COUNT,
        virus_count: VIRUS_COUNT,
        bot_count: BOT_COUNT,
        max_cells_per_player: MAX_CELLS_PER_PLAYER,
        min_split_mass: MIN_SPLIT_MASS,
        max_mass: MAX_MASS,
        merge_cooldown_ms: MERGE_COOLDOWN_MS,
        eat_ratio: EAT_RATIO,
        emote_slots: EMOTE_SLOTS,
        emote_unlock_mass: EMOTE_UNLOCK_MASS,
        emote_cooldown_ms: EMOTE_COOLDOWN_MS,
        emote_burst_unlock_mass_eaten: EMOTE_BURST_UNLOCK_MASS_EATEN,
        emote_burst_min_mass: EMOTE_BURST_MIN_MASS,
        emote_burst_cooldown_ms: EMOTE_BURST_COOLDOWN_MS,
        emote_burst_radius: EMOTE_BURST_RADIUS,
        emote_burst_damage: EMOTE_BURST_DAMAGE,
        emote_burst_knockback: EMOTE_BURST_KNOCKBACK,
        emote_burst_mass_cost: EMOTE_BURST_MASS_COST,
        max_name_len: MAX_NAME_LEN as u32,
        max_channel_len: MAX_CHANNEL_LEN as u32,
    });

    let mut rng = ctx.rng();
    for _ in 0..FOOD_COUNT {
        let (x, y) = random_map_point(ctx, &get_config(ctx));
        ctx.db.food().insert(Food {
            id: 0,
            x,
            y,
            mass: FOOD_MASS,
            color: rng.gen_range(0..u8::MAX),
        });
    }
    for _ in 0..VIRUS_COUNT {
        let (x, y) = random_map_point(ctx, &get_config(ctx));
        ctx.db.virus().insert(Virus { id: 0, x, y, mass: VIRUS_MASS });
    }
    for _ in 0..BOT_COUNT {
        spawn_bot(ctx);
    }

    // Start the fixed-step game loop.
    ctx.db.tick_schedule().insert(TickSchedule {
        scheduled_id: 0,
        scheduled_at: Duration::from_millis(TICK_MS).into(),
    });
}

#[reducer(client_connected)]
fn identity_connected(_ctx: &ReducerContext) {
    // Sessions are created on explicit `join`.
}

#[reducer(client_disconnected)]
fn identity_disconnected(ctx: &ReducerContext) {
    let sender = ctx.sender();
    if let Some(session) = ctx.db.session().identity().find(sender) {
        ctx.db.session().id().delete(session.id);
        kill_player(ctx, session.player_id, None, 0.0);
    }
}

// ---------------------------------------------------------------------------
// Player reducers
// ---------------------------------------------------------------------------

#[reducer]
fn join(ctx: &ReducerContext, name: String, channel: String) -> Result<(), String> {
    let sender = ctx.sender();
    if ctx.db.session().identity().find(sender).is_some() {
        return Err("already joined".into());
    }
    let cfg = get_config(ctx);
    let name = name.trim().to_string();
    let channel = channel.trim().to_lowercase();
    if name.is_empty() {
        return Err("name required".into());
    }
    if name.len() > cfg.max_name_len as usize {
        return Err(format!("name too long (max {})", cfg.max_name_len));
    }
    if channel.len() > cfg.max_channel_len as usize {
        return Err(format!("channel too long (max {})", cfg.max_channel_len));
    }

    let player_id = create_player(ctx, sender, name, channel, false);
    ctx.db.session().insert(Session {
        id: 0,
        identity: sender,
        player_id,
        connected: true,
    });
    Ok(())
}

#[reducer]
fn leave(ctx: &ReducerContext) -> Result<(), String> {
    let sender = ctx.sender();
    let session = ctx.db.session().identity().find(sender).ok_or("not joined")?;
    ctx.db.session().id().delete(session.id);
    kill_player(ctx, session.player_id, None, 0.0);
    Ok(())
}

#[reducer]
fn set_target(ctx: &ReducerContext, x: f32, y: f32) -> Result<(), String> {
    let sender = ctx.sender();
    let session = ctx.db.session().identity().find(sender).ok_or("not joined")?;
    let cfg = get_config(ctx);
    // Clamp the target into the map disc.
    let len = (x * x + y * y).sqrt();
    let (x, y) = if len > cfg.map_radius {
        (x / len * cfg.map_radius, y / len * cfg.map_radius)
    } else {
        (x, y)
    };
    for mut cell in player_cells(ctx, session.player_id) {
        cell.target_x = x;
        cell.target_y = y;
        ctx.db.cell().id().update(cell);
    }
    Ok(())
}

#[reducer]
fn split(ctx: &ReducerContext) -> Result<(), String> {
    let sender = ctx.sender();
    let session = ctx.db.session().identity().find(sender).ok_or("not joined")?;
    let cfg = get_config(ctx);
    let cells = player_cells(ctx, session.player_id);
    if cells.len() >= cfg.max_cells_per_player as usize {
        return Ok(());
    }

    let mut children = Vec::new();
    let mut halved: Vec<Cell> = Vec::new();
    for cell in &cells {
        if cell.mass < cfg.min_split_mass {
            continue;
        }
        if cells.len() + children.len() >= cfg.max_cells_per_player as usize {
            break;
        }
        let half = cell.mass / 2.0;
        // Aim the boost toward the player's current target.
        let dx = cell.target_x - cell.x;
        let dy = cell.target_y - cell.y;
        let len = (dx * dx + dy * dy).sqrt().max(1.0);
        let (ndx, ndy) = (dx / len, dy / len);
        children.push(Cell {
            id: 0,
            player_id: cell.player_id,
            x: cell.x + ndx * cell_radius(half),
            y: cell.y + ndy * cell_radius(half),
            mass: half,
            target_x: cell.target_x,
            target_y: cell.target_y,
            boost: SPLIT_BOOST,
            boost_dx: ndx,
            boost_dy: ndy,
            merge_at: now_plus_ms(ctx, cfg.merge_cooldown_ms),
        });
        let mut parent = cell.clone();
        parent.mass = half;
        parent.merge_at = now_plus_ms(ctx, cfg.merge_cooldown_ms);
        halved.push(parent);
    }
    for parent in halved {
        ctx.db.cell().id().update(parent);
    }
    for child in children {
        ctx.db.cell().insert(child);
    }
    Ok(())
}

#[reducer]
fn eject_mass(ctx: &ReducerContext, dx: f32, dy: f32) -> Result<(), String> {
    let sender = ctx.sender();
    let session = ctx.db.session().identity().find(sender).ok_or("not joined")?;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 0.001 {
        return Err("direction required".into());
    }
    let (ndx, ndy) = (dx / len, dy / len);
    for cell in player_cells(ctx, session.player_id) {
        if cell.mass < EJECT_MIN_MASS {
            continue;
        }
        ctx.db.ejected().insert(Ejected {
            id: 0,
            x: cell.x + ndx * (cell_radius(cell.mass) + 10.0),
            y: cell.y + ndy * (cell_radius(cell.mass) + 10.0),
            mass: EJECT_MASS,
            boost: EJECT_SPEED,
            boost_dx: ndx,
            boost_dy: ndy,
            owner_id: cell.player_id,
        });
        let mut updated = cell.clone();
        updated.mass -= EJECT_LOSS;
        ctx.db.cell().id().update(updated);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Emote reducers
// ---------------------------------------------------------------------------

/// Simulated Twitch import: replaces this player's channel emote set.
#[reducer]
fn import_emotes(ctx: &ReducerContext, pack: EmotePack) -> Result<(), String> {
    let sender = ctx.sender();
    let session = ctx.db.session().identity().find(sender).ok_or("not joined")?;
    let player = ctx.db.player().id().find(session.player_id).ok_or("player missing")?;
    let cfg = get_config(ctx);
    let channel = player.channel.clone();

    // Replace the channel's existing emote set.
    let existing: Vec<Emote> = ctx.db.emote().channel().filter(channel.as_str()).collect();
    for e in existing {
        ctx.db.emote().id().delete(e.id);
    }

    let mut next_slot: u32 = 0;
    for entry in pack.emotes.iter().take(MAX_EMOTES_PER_CHANNEL as usize) {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        // Accept "url" or "slot:url"; otherwise auto-assign slots in order.
        let (slot, url) = match entry.split_once(':') {
            Some((prefix, rest))
                if !prefix.is_empty()
                    && prefix.len() <= 2
                    && prefix.chars().all(|c| c.is_ascii_digit()) =>
            {
                (prefix.parse::<u32>().unwrap_or(next_slot), rest.trim().to_string())
            }
            _ => (next_slot, entry.to_string()),
        };
        if slot >= cfg.emote_slots || url.is_empty() {
            continue;
        }
        ctx.db.emote().insert(Emote {
            id: 0,
            channel: channel.clone(),
            slot,
            url,
        });
        next_slot += 1;
    }
    Ok(())
}

#[reducer]
fn show_emote(ctx: &ReducerContext, slot: u32) -> Result<(), String> {
    let sender = ctx.sender();
    let session = ctx.db.session().identity().find(sender).ok_or("not joined")?;
    let player = ctx.db.player().id().find(session.player_id).ok_or("player missing")?;
    let cfg = get_config(ctx);
    if slot >= cfg.emote_slots {
        return Err("invalid emote slot".into());
    }
    let emote = ctx
        .db
        .emote()
        .channel()
        .filter(player.channel.as_str())
        .find(|e| e.slot == slot)
        .ok_or("no emote in slot")?;

    // Gate on mass and cooldown using the player's biggest cell.
    let cells = player_cells(ctx, session.player_id);
    let Some(biggest) = cells.iter().max_by(|a, b| a.mass.total_cmp(&b.mass)) else {
        return Err("no cells".into());
    };
    if biggest.mass < cfg.emote_unlock_mass {
        return Err("need more mass to emote".into());
    }
    let cooldown_until =
        ctx.timestamp - Duration::from_millis(cfg.emote_cooldown_ms);
    if ctx
        .db
        .emote_event()
        .player_id()
        .filter(session.player_id)
        .any(|e| e.expires_at > cooldown_until)
    {
        return Err("emote on cooldown".into());
    }

    ctx.db.emote_event().insert(EmoteEvent {
        id: 0,
        player_id: session.player_id,
        cell_id: biggest.id,
        emote_id: emote.id,
        x: biggest.x,
        y: biggest.y,
        expires_at: now_plus_ms(ctx, EMOTE_DURATION_MS),
    });
    Ok(())
}

#[reducer]
fn burst_emote(ctx: &ReducerContext, slot: u32) -> Result<(), String> {
    let sender = ctx.sender();
    let session = ctx.db.session().identity().find(sender).ok_or("not joined")?;
    let player = ctx.db.player().id().find(session.player_id).ok_or("player missing")?;
    let cfg = get_config(ctx);
    if slot >= cfg.emote_slots {
        return Err("invalid emote slot".into());
    }
    if player.total_mass_eaten < cfg.emote_burst_unlock_mass_eaten {
        return Err("emote burst locked".into());
    }
    let emote = ctx
        .db
        .emote()
        .channel()
        .filter(player.channel.as_str())
        .find(|e| e.slot == slot)
        .ok_or("no emote in slot")?;

    let cells = player_cells(ctx, session.player_id);
    let Some(biggest) = cells.iter().max_by(|a, b| a.mass.total_cmp(&b.mass)) else {
        return Err("no cells".into());
    };
    if biggest.mass < cfg.emote_burst_min_mass {
        return Err("need more mass for burst".into());
    }
    let cooldown_until =
        ctx.timestamp - Duration::from_millis(cfg.emote_burst_cooldown_ms);
    if ctx
        .db
        .emote_burst()
        .player_id()
        .filter(session.player_id)
        .any(|b| b.expires_at > cooldown_until)
    {
        return Err("burst on cooldown".into());
    }

    // Pay the mass cost.
    let mut spent = biggest.clone();
    spent.mass *= 1.0 - cfg.emote_burst_mass_cost;
    ctx.db.cell().id().update(spent);

    ctx.db.emote_burst().insert(EmoteBurst {
        id: 0,
        player_id: session.player_id,
        cell_id: biggest.id,
        emote_id: emote.id,
        x: biggest.x,
        y: biggest.y,
        expires_at: now_plus_ms(ctx, EMOTE_DURATION_MS),
    });
    Ok(())
}

// ---------------------------------------------------------------------------
// Game loop
// ---------------------------------------------------------------------------

#[reducer]
fn tick(ctx: &ReducerContext, _arg: TickSchedule) -> Result<(), String> {
    // Only the scheduler (the database itself) may drive the loop.
    if ctx.sender() != ctx.database_identity() {
        return Err("tick may only be invoked by the scheduler".into());
    }
    let cfg = get_config(ctx);

    move_cells(ctx, &cfg);
    move_ejected(ctx);
    eat_food(ctx);
    eat_ejected(ctx);
    virus_collisions(ctx, &cfg);
    cell_collisions(ctx, &cfg);
    decay_mass(ctx, &cfg);
    bot_ai(ctx, &cfg);
    top_up_entities(ctx, &cfg);
    expire_effects(ctx);
    if (ctx.timestamp.to_micros_since_unix_epoch() as u64 / (cfg.tick_ms * 1000))
        % LEADERBOARD_INTERVAL_TICKS
        == 0
    {
        recompute_scoreboards(ctx);
    }
    Ok(())
}

fn move_cells(ctx: &ReducerContext, cfg: &Config) {
    let dt = cfg.tick_ms as f32 / 1000.0;
    let cells: Vec<Cell> = ctx.db.cell().iter().collect();
    for mut cell in cells {
        let speed = cell_speed(cell.mass);
        let dx = cell.target_x - cell.x;
        let dy = cell.target_y - cell.y;
        let len = (dx * dx + dy * dy).sqrt();
        let mut vx = 0.0;
        let mut vy = 0.0;
        if len > 1.0 {
            vx = dx / len * speed;
            vy = dy / len * speed;
        }
        if cell.boost > 0.0 {
            vx += cell.boost_dx * cell.boost;
            vy += cell.boost_dy * cell.boost;
            cell.boost = (cell.boost - 1400.0 * dt).max(0.0);
        }
        cell.x += vx * dt;
        cell.y += vy * dt;
        // Keep inside the map disc.
        let r = (cell.x * cell.x + cell.y * cell.y).sqrt();
        if r > cfg.map_radius {
            cell.x = cell.x / r * cfg.map_radius;
            cell.y = cell.y / r * cfg.map_radius;
        }
        ctx.db.cell().id().update(cell);
    }
}

fn move_ejected(ctx: &ReducerContext) {
    let dt = TICK_MS as f32 / 1000.0;
    let blobs: Vec<Ejected> = ctx.db.ejected().iter().collect();
    for mut e in blobs {
        if e.boost > 0.0 {
            e.x += e.boost_dx * e.boost * dt;
            e.y += e.boost_dy * e.boost * dt;
            e.boost = (e.boost - 1400.0 * dt).max(0.0);
            ctx.db.ejected().id().update(e);
        }
    }
}

fn eat_food(ctx: &ReducerContext) {
    let cells: Vec<Cell> = ctx.db.cell().iter().collect();
    let foods: Vec<Food> = ctx.db.food().iter().collect();
    for cell in &cells {
        let r = cell_radius(cell.mass);
        for food in &foods {
            if dist2(cell.x, cell.y, food.x, food.y) < r * r
                && ctx.db.food().id().delete(food.id)
            {
                gain_mass(ctx, cell.player_id, cell.id, food.mass);
            }
        }
    }
}

fn eat_ejected(ctx: &ReducerContext) {
    let cells: Vec<Cell> = ctx.db.cell().iter().collect();
    let blobs: Vec<Ejected> = ctx.db.ejected().iter().collect();
    for cell in &cells {
        let r = cell_radius(cell.mass);
        for blob in &blobs {
            if dist2(cell.x, cell.y, blob.x, blob.y) < r * r
                && ctx.db.ejected().id().delete(blob.id)
            {
                gain_mass(ctx, cell.player_id, cell.id, blob.mass);
            }
        }
    }
}

fn virus_collisions(ctx: &ReducerContext, cfg: &Config) {
    let cells: Vec<Cell> = ctx.db.cell().iter().collect();
    let viruses: Vec<Virus> = ctx.db.virus().iter().collect();
    for cell in &cells {
        for virus in &viruses {
            let r = cell_radius(cell.mass);
            if dist2(cell.x, cell.y, virus.x, virus.y) >= r * r {
                continue;
            }
            if !ctx.db.virus().id().delete(virus.id) {
                continue;
            }
            gain_mass(ctx, cell.player_id, cell.id, virus.mass);
            let my_cells = player_cells(ctx, cell.player_id).len() as u32;
            if cell.mass > VIRUS_SPLIT_MASS * 2.0 && my_cells < cfg.max_cells_per_player {
                let half = cell.mass / 2.0;
                let mut rng = ctx.rng();
                let angle: f32 = rng.gen_range(0.0..std::f32::consts::TAU);
                let (ndx, ndy) = (angle.cos(), angle.sin());
                ctx.db.cell().insert(Cell {
                    id: 0,
                    player_id: cell.player_id,
                    x: cell.x + ndx * cell_radius(half),
                    y: cell.y + ndy * cell_radius(half),
                    mass: half,
                    target_x: cell.target_x,
                    target_y: cell.target_y,
                    boost: SPLIT_BOOST,
                    boost_dx: ndx,
                    boost_dy: ndy,
                    merge_at: now_plus_ms(ctx, cfg.merge_cooldown_ms),
                });
                if let Some(mut parent) = ctx.db.cell().id().find(cell.id) {
                    parent.mass = half;
                    parent.merge_at = now_plus_ms(ctx, cfg.merge_cooldown_ms);
                    ctx.db.cell().id().update(parent);
                }
            }
        }
    }
}

fn cell_collisions(ctx: &ReducerContext, cfg: &Config) {
    let cells: Vec<Cell> = ctx.db.cell().iter().collect();
    for a in &cells {
        for b in &cells {
            if a.id >= b.id {
                continue;
            }
            // Skip pairs where either row was already eaten this tick.
            let (Some(a), Some(b)) = (
                ctx.db.cell().id().find(a.id),
                ctx.db.cell().id().find(b.id),
            ) else {
                continue;
            };
            let ra = cell_radius(a.mass);
            let rb = cell_radius(b.mass);
            let d = dist2(a.x, a.y, b.x, b.y).sqrt();
            if d >= ra + rb {
                continue;
            }
            if a.player_id == b.player_id {
                // Siblings merge only after cooldown and when well overlapped.
                if ctx.timestamp < a.merge_at || ctx.timestamp < b.merge_at {
                    continue;
                }
                if d < ra.max(rb) * 0.5 {
                    let (big, small) = if a.mass >= b.mass { (a, b) } else { (b, a) };
                    absorb(ctx, big.id, small.id, small.mass);
                }
            } else {
                let (big, small) = if a.mass >= b.mass { (a, b) } else { (b, a) };
                if big.mass < small.mass * cfg.eat_ratio {
                    continue;
                }
                // Eater must cover the victim's center.
                if d < cell_radius(big.mass) {
                    absorb(ctx, big.id, small.id, small.mass);
                    if player_cells(ctx, small.player_id).is_empty() {
                        kill_player(ctx, small.player_id, Some(big.player_id), small.mass);
                    }
                }
            }
        }
    }
}

fn absorb(ctx: &ReducerContext, eater_id: u64, eaten_id: u64, mass: f32) {
    if let Some(mut eater) = ctx.db.cell().id().find(eater_id) {
        eater.mass += mass;
        ctx.db.cell().id().update(eater);
    }
    ctx.db.cell().id().delete(eaten_id);
}

fn gain_mass(ctx: &ReducerContext, player_id: u64, cell_id: u64, mass: f32) {
    if let Some(mut cell) = ctx.db.cell().id().find(cell_id) {
        cell.mass = (cell.mass + mass).min(MAX_MASS);
        ctx.db.cell().id().update(cell);
    }
    if let Some(mut p) = ctx.db.player().id().find(player_id) {
        p.total_mass_eaten += mass;
        ctx.db.player().id().update(p);
    }
}

fn decay_mass(ctx: &ReducerContext, cfg: &Config) {
    let dt = cfg.tick_ms as f32 / 1000.0;
    let cells: Vec<Cell> = ctx.db.cell().iter().collect();
    for mut cell in cells {
        if cell.mass > 100.0 {
            cell.mass = (cell.mass * (1.0 - DECAY_RATE * dt)).max(20.0);
            ctx.db.cell().id().update(cell);
        }
    }
}

fn bot_ai(ctx: &ReducerContext, cfg: &Config) {
    let mut rng = ctx.rng();
    let bots: Vec<Player> = ctx
        .db
        .player()
        .iter()
        .filter(|p| p.is_bot && p.alive)
        .collect();
    for bot in bots {
        let cells = player_cells(ctx, bot.id);
        let Some(me) = cells.iter().max_by(|a, b| a.mass.total_cmp(&b.mass)) else {
            continue;
        };

        // Threat scan: nearest bigger enemy → flee; best-value smaller enemy → chase.
        let mut threat: Option<(f32, f32, f32)> = None; // (dist, x, y)
        let mut best_prey: Option<(f32, f32, f32)> = None; // (score, x, y)
        for other in ctx.db.cell().iter() {
            if other.player_id == bot.id {
                continue;
            }
            let d = dist2(me.x, me.y, other.x, other.y).sqrt();
            if d > 900.0 {
                continue;
            }
            if other.mass > me.mass * cfg.eat_ratio {
                if threat.map_or(true, |(td, _, _)| d < td) {
                    threat = Some((d, other.x, other.y));
                }
            } else if me.mass > other.mass * cfg.eat_ratio {
                let score = other.mass / (d + 50.0);
                if best_prey.map_or(true, |(s, _, _)| score > s) {
                    best_prey = Some((score, other.x, other.y));
                }
            }
        }

        let (tx, ty) = if let Some((_, ex, ey)) = threat {
            // Flee directly away from the threat.
            let dx = me.x - ex;
            let dy = me.y - ey;
            let len = (dx * dx + dy * dy).sqrt().max(1.0);
            (
                (me.x + dx / len * 600.0).clamp(-cfg.map_radius, cfg.map_radius),
                (me.y + dy / len * 600.0).clamp(-cfg.map_radius, cfg.map_radius),
            )
        } else if let Some((_, px, py)) = best_prey {
            (px, py)
        } else {
            // Wander toward the nearest food.
            let mut nearest: Option<(f32, f32, f32)> = None; // (dist2, x, y)
            for food in ctx.db.food().iter() {
                let d = dist2(me.x, me.y, food.x, food.y);
                if nearest.map_or(true, |(nd, _, _)| d < nd) {
                    nearest = Some((d, food.x, food.y));
                }
            }
            match nearest {
                Some((_, fx, fy)) => (fx, fy),
                None => {
                    let angle: f32 = rng.gen_range(0.0..std::f32::consts::TAU);
                    (me.x + angle.cos() * 300.0, me.y + angle.sin() * 300.0)
                }
            }
        };
        for mut cell in cells {
            cell.target_x = tx;
            cell.target_y = ty;
            ctx.db.cell().id().update(cell);
        }
    }

    // Respawn dead bots (one per tick until the population is restored).
    if alive_bot_count(ctx) < cfg.bot_count {
        spawn_bot(ctx);
    }
}

fn top_up_entities(ctx: &ReducerContext, cfg: &Config) {
    let mut rng = ctx.rng();
    let food_n = ctx.db.food().count();
    if food_n < cfg.food_count as u64 {
        for _ in 0..(cfg.food_count as u64 - food_n).min(20) {
            let (x, y) = random_map_point(ctx, cfg);
            ctx.db.food().insert(Food {
                id: 0,
                x,
                y,
                mass: FOOD_MASS,
                color: rng.gen_range(0..u8::MAX),
            });
        }
    }
    if ctx.db.virus().count() < cfg.virus_count as u64 {
        let (x, y) = random_map_point(ctx, cfg);
        ctx.db.virus().insert(Virus { id: 0, x, y, mass: VIRUS_MASS });
    }
}

fn expire_effects(ctx: &ReducerContext) {
    let now = ctx.timestamp;
    let events: Vec<EmoteEvent> = ctx
        .db
        .emote_event()
        .iter()
        .filter(|e| e.expires_at <= now)
        .collect();
    for e in events {
        ctx.db.emote_event().id().delete(e.id);
    }
    let bursts: Vec<EmoteBurst> = ctx
        .db
        .emote_burst()
        .iter()
        .filter(|b| b.expires_at <= now)
        .collect();
    for b in bursts {
        ctx.db.emote_burst().id().delete(b.id);
    }
    // Trim the kill feed to the most recent entries.
    let mut feed: Vec<KillFeed> = ctx.db.kill_feed().iter().collect();
    if feed.len() > KILL_FEED_MAX {
        feed.sort_by(|a, b| b.at.cmp(&a.at));
        for old in feed.into_iter().skip(KILL_FEED_MAX) {
            ctx.db.kill_feed().id().delete(old.id);
        }
    }
}

fn recompute_scoreboards(ctx: &ReducerContext) {
    // Total mass per player.
    let mut totals: Vec<(u64, f32)> = Vec::new();
    for cell in ctx.db.cell().iter() {
        match totals.iter_mut().find(|(id, _)| *id == cell.player_id) {
            Some((_, m)) => *m += cell.mass,
            None => totals.push((cell.player_id, cell.mass)),
        }
    }
    totals.sort_by(|a, b| b.1.total_cmp(&a.1));

    ctx.db.leaderboard().clear();
    for (rank, (player_id, mass)) in totals.iter().take(LEADERBOARD_SIZE).enumerate() {
        if let Some(p) = ctx.db.player().id().find(*player_id) {
            ctx.db.leaderboard().insert(Leaderboard {
                rank: rank as u32 + 1,
                player_id: p.id,
                name: p.name.clone(),
                channel: p.channel.clone(),
                mass: *mass,
            });
        }
    }

    // Channel stats: aggregate mass and headcount per channel.
    let mut channels: std::collections::HashMap<String, (f32, u32)> =
        std::collections::HashMap::new();
    for (player_id, mass) in &totals {
        if let Some(p) = ctx.db.player().id().find(*player_id) {
            let entry = channels.entry(p.channel.clone()).or_insert((0.0, 0));
            entry.0 += mass;
            entry.1 += 1;
        }
    }
    ctx.db.channel_stats().clear();
    for (channel, (mass, count)) in channels {
        ctx.db.channel_stats().insert(ChannelStats {
            channel,
            total_mass: mass,
            player_count: count,
            kills: 0,
        });
    }
    // Re-apply kill counts from the feed.
    for kf in ctx.db.kill_feed().iter() {
        if let Some(mut stats) =
            ctx.db.channel_stats().channel().find(kf.killer_channel)
        {
            stats.kills += 1;
            ctx.db.channel_stats().channel().update(stats);
        }
    }
}