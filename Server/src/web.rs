use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use log::info;
use poker::game::Game;
use poker::games::{
  fivecarddraw::FiveCardDraw, sevencardstud::SevenCardStud, texasholdem::TexasHoldem,
};
use poker::hand::Hand;
use poker::player::Player;
use serde::Deserialize;
use serde_json::{json, Value};
use server::db::DbRepo;
use server::Table;
use tokio::sync::broadcast;

type SharedTable<G> = Arc<Mutex<Table<G, DbRepo>>>;

#[derive(Clone)]
pub struct WebState {
  tables: Arc<HashMap<String, WebTable>>,
  updates: broadcast::Sender<()>,
}

#[derive(Clone)]
enum WebTable {
  TexasHoldem(SharedTable<TexasHoldem>),
  FiveCardDraw(SharedTable<FiveCardDraw>),
  SevenCardStud(SharedTable<SevenCardStud>),
}

#[derive(Deserialize)]
struct JoinRequest {
  name: String,
  wallet: Option<u64>,
}

#[derive(Deserialize)]
struct ActionRequest {
  command: String,
}

#[derive(Deserialize)]
struct StateQuery {
  player_id: Option<u64>,
}

pub async fn serve(db: Arc<DbRepo>) {
  let state = WebState::new(db);
  let app = Router::new()
    .route("/", get(index))
    .route("/ws", get(websocket))
    .route("/api/games", get(list_games))
    .route("/api/games/:game/state", get(game_state))
    .route("/api/games/:game/join", post(join_game))
    .route(
      "/api/games/:game/players/:player_id/action",
      post(player_action),
    )
    .with_state(state);

  let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
  let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
  info!("Browser poker server listening on http://{}", addr);
  axum::serve(listener, app).await.unwrap();
}

impl WebState {
  fn new(db: Arc<DbRepo>) -> Self {
    let mut tables = HashMap::new();
    let (updates, _) = broadcast::channel(128);

    tables.insert(
      "TexasHoldem".to_string(),
      WebTable::TexasHoldem(new_web_table::<TexasHoldem>(
        "224.0.1.1:11000",
        db.clone(),
        updates.clone(),
      )),
    );
    tables.insert(
      "FiveCardDraw".to_string(),
      WebTable::FiveCardDraw(new_web_table::<FiveCardDraw>(
        "224.0.1.2:11001",
        db.clone(),
        updates.clone(),
      )),
    );
    tables.insert(
      "SevenCardStud".to_string(),
      WebTable::SevenCardStud(new_web_table::<SevenCardStud>(
        "224.0.1.3:11002",
        db,
        updates.clone(),
      )),
    );

    Self {
      tables: Arc::new(tables),
      updates,
    }
  }

  fn table(&self, game: &str) -> Option<&WebTable> {
    self.tables.get(game)
  }

  fn notify(&self) {
    let _ = self.updates.send(());
  }
}

fn new_web_table<G: Game + Send + 'static>(
  udp_addr: &str,
  db: Arc<DbRepo>,
  updates: broadcast::Sender<()>,
) -> SharedTable<G> {
  let table = Arc::new(Mutex::new(Table::<G, DbRepo>::new(udp_addr, db)));
  let monitor_table = table.clone();

  thread::spawn(move || loop {
    let should_start = {
      let table = monitor_table.lock().unwrap();
      table.get_num_players() >= 2 && !table.is_running()
    };

    if should_start {
      let has_showdown = {
        let table = monitor_table.lock().unwrap();
        table.showdown.is_some()
      };

      if has_showdown {
        thread::sleep(Duration::from_secs(10));
        let mut table = monitor_table.lock().unwrap();
        table.remove_unconfirmed_players();
        let _ = updates.send(());
      }

      let mut table = monitor_table.lock().unwrap();
      if table.get_num_players() >= 2 && !table.is_running() {
        table.start_game();
        let _ = updates.send(());
      }
    }

    thread::sleep(Duration::from_secs(1));
  });

  table
}

async fn index() -> Html<&'static str> {
  Html(INDEX_HTML)
}

async fn websocket(ws: WebSocketUpgrade, State(state): State<WebState>) -> impl IntoResponse {
  ws.on_upgrade(move |socket| websocket_session(socket, state.updates.subscribe()))
}

async fn websocket_session(mut socket: WebSocket, mut updates: broadcast::Receiver<()>) {
  let _ = socket.send(Message::Text("refresh".into())).await;

  loop {
    tokio::select! {
      update = updates.recv() => {
        if update.is_err() {
          break;
        }

        if socket.send(Message::Text("refresh".into())).await.is_err() {
          break;
        }
      }
      message = socket.recv() => {
        match message {
          Some(Ok(Message::Close(_))) | None => break,
          Some(Err(_)) => break,
          _ => {}
        }
      }
    }
  }
}

async fn list_games(State(state): State<WebState>) -> Json<Value> {
  let mut games = state.tables.keys().cloned().collect::<Vec<_>>();
  games.sort();
  Json(json!({ "games": games }))
}

async fn game_state(
  State(state): State<WebState>,
  Path(game): Path<String>,
  headers: HeaderMap,
  Query(query): Query<StateQuery>,
) -> impl IntoResponse {
  let Some(table) = state.table(&game) else {
    return json_error(StatusCode::NOT_FOUND, "Game not found");
  };

  let player_id = query.player_id.or_else(|| {
    headers
      .get(COOKIE)
      .and_then(|value| value.to_str().ok())
      .and_then(player_id_from_cookie)
  });

  Json(json!({
    "ok": true,
    "state": table.state(player_id),
  }))
  .into_response()
}

async fn join_game(
  State(state): State<WebState>,
  Path(game): Path<String>,
  headers: HeaderMap,
  Json(req): Json<JoinRequest>,
) -> impl IntoResponse {
  let Some(table) = state.table(&game) else {
    return json_error(StatusCode::NOT_FOUND, "Game not found");
  };

  if req.name.trim().is_empty() {
    return json_error(StatusCode::BAD_REQUEST, "Name is required");
  }

  let (player_id, set_cookie) = session_player_id(&headers);

  match table.join(player_id, req).await {
    Ok(message) => {
      state.notify();
      let mut response = Json(json!({
        "ok": true,
        "message": message,
        "player_id": player_id,
        "state": table.state(Some(player_id)),
      }))
      .into_response();
      if set_cookie {
        response.headers_mut().insert(SET_COOKIE, session_cookie(player_id));
      }
      response
    }
    Err(message) => json_error(StatusCode::BAD_REQUEST, &message),
  }
}

async fn player_action(
  State(state): State<WebState>,
  Path((game, player_id)): Path<(String, u64)>,
  Json(req): Json<ActionRequest>,
) -> impl IntoResponse {
  let Some(table) = state.table(&game) else {
    return json_error(StatusCode::NOT_FOUND, "Game not found");
  };

  match table.action(player_id, &req.command) {
    Ok(message) => {
      state.notify();
      let mut response = Json(json!({
        "ok": true,
        "message": message,
        "state": table.state(Some(player_id)),
      }))
      .into_response();
      if req.command.trim() == "LEAVE" {
        response.headers_mut().insert(SET_COOKIE, expired_session_cookie());
      }
      response
    }
    Err(message) => json_error(StatusCode::BAD_REQUEST, &message),
  }
}

impl WebTable {
  async fn join(&self, player_id: u64, req: JoinRequest) -> Result<String, String> {
    match self {
      WebTable::TexasHoldem(table) => join_table(table, player_id, req).await,
      WebTable::FiveCardDraw(table) => join_table(table, player_id, req).await,
      WebTable::SevenCardStud(table) => join_table(table, player_id, req).await,
    }
  }

  fn action(&self, player_id: u64, command: &str) -> Result<String, String> {
    match self {
      WebTable::TexasHoldem(table) => run_action(table, player_id, command),
      WebTable::FiveCardDraw(table) => run_action(table, player_id, command),
      WebTable::SevenCardStud(table) => run_action(table, player_id, command),
    }
  }

  fn state(&self, player_id: Option<u64>) -> Value {
    match self {
      WebTable::TexasHoldem(table) => table.lock().unwrap().player_view_state(player_id),
      WebTable::FiveCardDraw(table) => table.lock().unwrap().player_view_state(player_id),
      WebTable::SevenCardStud(table) => table.lock().unwrap().player_view_state(player_id),
    }
  }
}

async fn join_table<G: Game + Send + 'static>(
  table: &SharedTable<G>,
  player_id: u64,
  req: JoinRequest,
) -> Result<String, String> {
  let db = table.lock().unwrap().db.clone();
  let player = if let Some(mut db_player) = db.fetch_player(player_id).await {
    db_player.name = req.name;
    db.upsert_player(&db_player).await;
    db_player
  } else {
    let new_player = Player {
      id: player_id,
      name: req.name,
      wallet: req.wallet.unwrap_or(1000),
      bet: 0,
      folded: false,
      hand: Hand(Vec::new()),
    };
    db.upsert_player(&new_player).await;
    new_player
  };

  table.lock().unwrap().add_player(player)
}

fn session_player_id(headers: &HeaderMap) -> (u64, bool) {
  if let Some(id) = headers
    .get(COOKIE)
    .and_then(|value| value.to_str().ok())
    .and_then(player_id_from_cookie)
  {
    return (id, false);
  }

  (new_player_id(), true)
}

fn player_id_from_cookie(cookie_header: &str) -> Option<u64> {
  cookie_header
    .split(';')
    .filter_map(|part| part.trim().split_once('='))
    .find_map(|(name, value)| {
      if name == "poker_session" {
        value.parse::<u64>().ok().filter(|id| *id > 0)
      } else {
        None
      }
    })
}

fn new_player_id() -> u64 {
  const MAX_SAFE_JS_INTEGER: u64 = 9_007_199_254_740_991;
  let bytes = *uuid::Uuid::new_v4().as_bytes();
  let mut id = u64::from_be_bytes(bytes[..8].try_into().unwrap()) % MAX_SAFE_JS_INTEGER;
  if id == 0 {
    id = 1;
  }
  id
}

fn session_cookie(player_id: u64) -> HeaderValue {
  HeaderValue::from_str(&format!(
    "poker_session={}; Path=/; HttpOnly; SameSite=Lax",
    player_id
  ))
  .expect("session cookie should be a valid header")
}

fn expired_session_cookie() -> HeaderValue {
  HeaderValue::from_static("poker_session=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax")
}

fn run_action<G: Game + Send + 'static>(
  table: &SharedTable<G>,
  player_id: u64,
  command: &str,
) -> Result<String, String> {
  let command = command.trim();
  if command.is_empty() {
    return Err("Command is required".to_string());
  }

  let parts = command.split_whitespace().collect::<Vec<_>>();

  if parts == ["LEAVE"] {
    return table.lock().unwrap().remove_player(player_id);
  }

  if parts == ["UPDATE"] {
    return table.lock().unwrap().player_send(player_id);
  }

  if parts == ["KEEP_PLAYING"] {
    return table.lock().unwrap().keep_playing(player_id);
  }

  let turn_id = table
    .lock()
    .unwrap()
    .turn
    .as_ref()
    .map(|node| node.lock().unwrap().player.id);
  if Some(player_id) != turn_id {
    return Err("[ERROR] NOT YOUR TURN".to_string());
  }

  {
    let mut table = table.lock().unwrap();
    match table.game_actions(player_id, &parts) {
      Ok(message) => {
        table.advance_turn();
        return Ok(message);
      }
      Err(message) if message == "[BET]" => {}
      Err(message) => return Err(message),
    }
  }

  let result = match parts.as_slice() {
    ["RAISE", amount] => {
      let amount = amount
        .parse::<u64>()
        .map_err(|_| "[ERROR] INVALID AMOUNT".to_string())?;
      table.lock().unwrap().raise(player_id, amount)
    }
    ["CHECK"] => table.lock().unwrap().check(player_id),
    ["FOLD"] => table.lock().unwrap().fold(player_id),
    _ => Err("[ERROR] INVALID COMMAND".to_string()),
  };

  if result.is_ok() {
    table.lock().unwrap().advance_turn();
  }

  result
}

fn json_error(status: StatusCode, message: &str) -> axum::response::Response {
  (status, Json(json!({ "ok": false, "error": message }))).into_response()
}

const INDEX_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Poker</title>
  <style>
  :root { color-scheme: dark; font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
  body { margin: 0; min-height: 100vh; background: radial-gradient(circle at 50% 12%, #315957 0, #18292b 45%, #0d1719 100%); color: #f6f1df; }
  header { display: flex; align-items: center; justify-content: space-between; gap: 16px; padding: 18px 24px; background: rgba(9, 20, 22, .84); border-bottom: 1px solid rgba(255,255,255,.08); }
  h1 { margin: 0; font-size: 22px; font-weight: 800; letter-spacing: 0; }
  h2 { margin: 0; font-size: 14px; text-transform: uppercase; color: #d9c27a; }
  main { display: grid; grid-template-columns: 300px 1fr; gap: 24px; padding: 24px; max-width: 1220px; margin: 0 auto; }
  section, aside { min-width: 0; }
  .is-hidden { display: none !important; }
  .start-view { max-width: 430px; margin: 72px auto 0; display: grid; gap: 16px; }
  .start-view h2 { font-size: 24px; text-transform: none; color: #fff7d8; }
  .panel { background: rgba(13, 27, 29, .82); border: 1px solid rgba(255,255,255,.1); border-radius: 8px; padding: 16px; box-shadow: 0 16px 40px rgba(0,0,0,.22); }
  .stack { display: grid; gap: 12px; }
  label { display: grid; gap: 6px; font-size: 12px; font-weight: 700; color: #d3dacd; }
  input, select { height: 38px; border: 1px solid rgba(255,255,255,.18); border-radius: 6px; padding: 0 10px; font: inherit; background: #122528; color: #f6f1df; }
  input[readonly] { color: #98aaa6; background: #0d1a1c; }
  button { height: 38px; border: 0; border-radius: 6px; padding: 0 12px; background: #d9b64c; color: #161916; font: inherit; font-weight: 800; cursor: pointer; }
  button.secondary { background: #244f52; color: #f6f1df; }
  button.danger { background: #9f403e; color: #fff4ec; }
  button:disabled { opacity: .5; cursor: default; }
  .row { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
  .actions { display: grid; gap: 12px; }
  .table { display: grid; gap: 16px; }
  .summary { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 12px; }
  .metric { background: rgba(255,255,255,.08); border: 1px solid rgba(255,255,255,.08); border-radius: 8px; padding: 12px; min-height: 58px; }
  .metric span { display: block; font-size: 12px; color: #b4c5be; }
  .metric strong { display: block; margin-top: 4px; font-size: 18px; }
  .felt { position: relative; min-height: 560px; overflow: hidden; border-radius: 180px; padding: 34px; background: radial-gradient(ellipse at center, #2f7b63 0, #1d634f 58%, #134337 100%); border: 18px solid #4a2d1f; box-shadow: inset 0 0 0 8px rgba(214,174,85,.55), inset 0 20px 70px rgba(255,255,255,.08), 0 24px 80px rgba(0,0,0,.36); }
  .felt::before { content: ""; position: absolute; inset: 24px; border: 2px solid rgba(242,224,156,.24); border-radius: 160px; pointer-events: none; }
  .board-panel { position: relative; z-index: 1; display: grid; justify-items: center; align-content: center; gap: 12px; min-height: 210px; margin: 72px auto 28px; max-width: 440px; padding: 20px; border-radius: 8px; background: rgba(10,40,34,.28); }
  .pot-center { display: grid; justify-items: center; gap: 8px; min-width: 170px; padding: 10px 16px; border: 1px solid rgba(240,211,109,.42); border-radius: 8px; background: rgba(8,28,25,.78); box-shadow: 0 12px 28px rgba(0,0,0,.24); }
  .pot-center span { color: #d9c27a; font-size: 12px; font-weight: 800; text-transform: uppercase; }
  .pot-center strong { color: #fff7d8; font-size: 24px; line-height: 1; }
  .players { position: relative; z-index: 1; display: grid; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr)); gap: 12px; }
  .player { border: 1px solid rgba(255,255,255,.12); border-radius: 8px; padding: 12px; background: rgba(7,19,20,.78); color: #f6f1df; box-shadow: 0 10px 28px rgba(0,0,0,.22); }
  .player.self { border-color: rgba(217,182,76,.58); background: rgba(25,42,35,.86); }
  .player.current { border-color: #d9b64c; box-shadow: inset 0 0 0 1px #d9b64c, 0 10px 28px rgba(0,0,0,.22); }
  .player.winner { border-color: #f0d36d; box-shadow: inset 0 0 0 2px #f0d36d, 0 0 28px rgba(240,211,109,.28), 0 10px 28px rgba(0,0,0,.22); }
  .player h3 { margin: 0 0 8px; font-size: 15px; color: #fff7d8; }
  .me-tag { color: #f0d36d; font-size: 12px; font-weight: 900; }
  .chip-row { display: grid; gap: 5px; margin-top: 8px; min-height: 34px; }
  .money-label { color: #d3dacd; font-size: 12px; font-weight: 800; }
  .money-chips { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; min-height: 30px; }
  .chip-unit { display: inline-flex; align-items: center; gap: 4px; }
  .chip { width: 30px; height: 30px; border-radius: 50%; display: inline-grid; place-items: center; color: #fff; border: 3px dashed rgba(255,255,255,.78); box-shadow: inset 0 0 0 4px rgba(0,0,0,.16), 0 4px 8px rgba(0,0,0,.28); font-size: 10px; font-weight: 900; line-height: 1; }
  .chip { background: #59656c; }
  .chip.denom-10 { background: #336fa3; }
  .chip.denom-25 { background: #2f8c62; }
  .chip.denom-100 { background: #b93c3c; }
  .chip.denom-500 { background: #d9b64c; color: #1b1710; }
  .chip-count { color: #d3dacd; font-size: 11px; font-weight: 800; }
  .raise-builder { display: grid; gap: 10px; }
  .raise-preview { display: grid; gap: 6px; min-height: 48px; padding: 8px; border-radius: 6px; background: rgba(255,255,255,.07); }
  .chip-picker { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 8px; }
  .chip-button { height: 46px; display: grid; place-items: center; padding: 0; background: transparent; }
  .chip-button .chip { width: 36px; height: 36px; }
  .players .cards { margin-top: 10px; }
  .rank-used.cards { min-height: 34px; }
  .rank-used .card { width: 28px; height: 38px; font-size: 12px; }
  .cards { display: flex; gap: 6px; flex-wrap: wrap; min-height: 44px; }
  .board { align-items: center; min-height: 54px; }
  .empty-board { color: #c9d3be; font-size: 14px; }
  .card { width: 42px; height: 58px; border: 1px solid #e5ddc8; border-radius: 7px; display: inline-grid; place-items: center; background: linear-gradient(145deg, #fff 0, #f3efe5 100%); color: #20242a; font-size: 17px; font-weight: 900; line-height: 1; box-shadow: 0 8px 14px rgba(0,0,0,.24); }
  .card.selectable { cursor: pointer; transition: transform .12s ease, box-shadow .12s ease; }
  .card.selectable:hover { transform: translateY(-4px); }
  .card.selected { transform: translateY(-8px); box-shadow: 0 0 0 3px #d9b64c, 0 12px 18px rgba(0,0,0,.3); }
  .card.rank-card { box-shadow: 0 0 0 3px #f0d36d, 0 12px 18px rgba(0,0,0,.3); }
  .card.red { color: #b72f3a; border-color: #e1a6ad; }
  .card.hidden { color: transparent; background: repeating-linear-gradient(45deg, #244f52 0 5px, #19383b 5px 10px); border-color: #8bb7ad; }
  .hint { color: #b4c5be; font-size: 12px; line-height: 1.35; }
  .rank-label { margin-top: 8px; color: #f0d36d; font-size: 12px; font-weight: 800; }
  .modal-backdrop { position: fixed; inset: 0; z-index: 20; display: grid; place-items: center; padding: 20px; background: rgba(3, 9, 10, .72); }
  .modal { width: min(420px, 100%); display: grid; gap: 16px; background: #102326; border: 1px solid rgba(255,255,255,.16); border-radius: 8px; padding: 20px; box-shadow: 0 24px 80px rgba(0,0,0,.45); }
  .modal h2 { font-size: 22px; text-transform: none; color: #fff7d8; }
  @media (max-width: 760px) { main { grid-template-columns: 1fr; padding: 16px; } header { padding: 14px 16px; } .summary { grid-template-columns: 1fr; } .felt { min-height: 520px; border-width: 10px; border-radius: 80px; padding: 18px; } .board-panel { margin-top: 28px; } }
  </style>
</head>
<body>
  <header>
  <h1>Poker</h1>
  <div id="connection">Disconnected</div>
  </header>
  <section id="startView" class="panel start-view">
  <h2>Choose a table</h2>
  <label>Game <select id="game"></select></label>
  <input id="playerId" type="hidden">
  <label>Name <input id="name" value="Player"></label>
  <button id="join">Join Table</button>
  </section>
  <main id="gameView" class="is-hidden">
  <aside class="panel stack">
    <h2 id="gameTitle">Table</h2>
    <div id="waitingHint" class="hint">Waiting for your turn.</div>
    <div id="betActions" class="actions">
    <div class="row">
      <button id="check" class="secondary">Check</button>
      <button id="fold" class="danger">Fold</button>
    </div>
    <div class="raise-builder">
      <div id="raisePreview" class="raise-preview"></div>
      <div id="raiseChips" class="chip-picker">
      <button class="chip-button" data-chip="10" type="button"><span class="chip denom-10">10</span></button>
      <button class="chip-button" data-chip="25" type="button"><span class="chip denom-25">25</span></button>
      <button class="chip-button" data-chip="100" type="button"><span class="chip denom-100">100</span></button>
      <button class="chip-button" data-chip="500" type="button"><span class="chip denom-500">500</span></button>
      </div>
      <div class="row">
      <button id="clearRaise" class="secondary" type="button">Clear</button>
      <button id="raise" type="button" disabled>Raise</button>
      </div>
    </div>
    </div>
    <div id="replaceActions" class="actions is-hidden">
    <button id="replaceCards" class="secondary" disabled>Replace Selected</button>
    <button id="standPat" class="secondary">Stand Pat</button>
    <div class="hint">Select cards from your hand to replace them.</div>
    </div>
    <button id="leaveTable" class="danger">Leave Table</button>
  </aside>
  <section class="table">
    <div class="summary">
    <div class="metric"><span>Turn</span><strong id="turn">-</strong></div>
    <div class="metric"><span>Round</span><strong id="round">-</strong></div>
    </div>
    <div class="felt">
    <div id="players" class="players"></div>
    <div class="board-panel">
      <div class="pot-center"><span>Pot</span><strong id="potTotal">0</strong><div id="potChips" class="money-chips"></div></div>
      <h2>Table Cards</h2>
      <div id="board" class="cards board"></div>
    </div>
    </div>
  </section>
  </main>
  <div id="gameOverModal" class="modal-backdrop is-hidden">
  <div class="modal">
    <h2>Keep playing?</h2>
    <div class="hint">This hand is over. Stay seated for the next hand or leave the table.</div>
    <div class="row">
    <button id="keepPlaying">Keep Playing</button>
    <button id="leaveAfterGame" class="danger">Leave Table</button>
    </div>
  </div>
  </div>
  <script>
  const game = document.querySelector("#game");
  const playerId = document.querySelector("#playerId");
  const nameInput = document.querySelector("#name");
  const connection = document.querySelector("#connection");
  let joined = false;
  let socket = null;
  let refreshInFlight = false;
  let refreshQueued = false;
  let selectedCards = new Set();
  let pendingRaise = 0;
  let hasSeenRunning = false;
  let gameOverPromptShown = false;
  let showdownKey = "";
  let showdownPromptTimer = null;

  function ensureClientName() {
    if (!localStorage.getItem("pokerClientName")) {
    localStorage.setItem("pokerClientName", "Player");
    }
    playerId.value = "";
    playerId.placeholder = "Assigned on join";
    nameInput.value = localStorage.getItem("pokerClientName");
  }

  async function api(path, options = {}) {
    const response = await fetch(path, {
    headers: { "content-type": "application/json" },
    ...options,
    });
    const body = await response.json();
    if (!response.ok || body.ok === false) throw new Error(body.error || "Request failed");
    return body;
  }

  function cardLabel(value) {
    const rank = value & 0x0f;
    const suit = (value >> 4) & 0x03;
    const suits = ["♦", "♣", "♥", "♠"];
    const labels = {1: "A", 11: "J", 12: "Q", 13: "K"};
    return `${labels[rank] || rank}${suits[suit] || ""}`;
  }

  function cardClass(value) {
    if (!value) return "card hidden";
    const suit = (value >> 4) & 0x03;
    return `card ${suit === 0 || suit === 2 ? "red" : ""}`;
  }

  function escapeHtml(value) {
    return String(value).replace(/[&<>"']/g, char => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    '"': "&quot;",
    "'": "&#39;",
    }[char]));
  }

  function chipBreakdown(amount) {
    const total = Math.max(0, Number(amount || 0));
    const denominations = [500, 100, 25, 10];
    const best = Array(total + 1).fill(null);
    best[0] = [];

    for (let value = 1; value <= total; value += 1) {
    denominations.forEach(denomination => {
      if (value >= denomination && best[value - denomination]) {
      const candidate = [...best[value - denomination], denomination];
      if (!best[value] || candidate.length < best[value].length) {
        best[value] = candidate;
      }
      }
    });
    }

    const values = best[total] || (total ? [total] : []);
    const counts = values.reduce((acc, value) => {
    acc[value] = (acc[value] || 0) + 1;
    return acc;
    }, {});
    return Object.entries(counts)
    .map(([value, count]) => ({ value: Number(value), count }))
    .sort((a, b) => b.value - a.value);
  }

  function renderMoney(target, label, amount) {
    const total = Number(amount || 0);
    const chips = chipBreakdown(total);
    target.innerHTML = `<div class="money-label">${label}: ${total}</div>
    <div class="money-chips">
      ${chips.length ? chips.map(chip => `<span class="chip-unit"><span class="chip denom-${chip.value}">${chip.value}</span>${chip.count > 1 ? `<span class="chip-count">x${chip.count}</span>` : ""}</span>`).join("") : `<span class="chip-count">0</span>`}
    </div>`;
  }

  function renderPot(amount) {
    document.querySelector("#potTotal").textContent = amount ?? 0;
    const potChips = document.querySelector("#potChips");
    const chips = chipBreakdown(amount);
    potChips.innerHTML = chips.length
    ? chips.map(chip => `<span class="chip-unit"><span class="chip denom-${chip.value}">${chip.value}</span>${chip.count > 1 ? `<span class="chip-count">x${chip.count}</span>` : ""}</span>`).join("")
    : `<span class="chip-count">0</span>`;
  }

  function updateRaisePreview() {
    renderMoney(document.querySelector("#raisePreview"), "Raise", pendingRaise);
    document.querySelector("#raise").disabled = pendingRaise <= 0;
  }

  function addRaise(amount) {
    pendingRaise += amount;
    updateRaisePreview();
  }

  function clearRaise() {
    pendingRaise = 0;
    updateRaisePreview();
  }

  function renderCards(target, cards = [], options = {}) {
    target.innerHTML = "";
    cards.forEach(card => {
    const el = document.createElement("span");
    const selectable = options.selectable && card;
    const selected = selectable && selectedCards.has(card);
    const rankCard = card && options.rankCards?.has(card);
    el.className = `${cardClass(card)} ${selectable ? "selectable" : ""} ${selected ? "selected" : ""} ${rankCard ? "rank-card" : ""}`;
    el.textContent = card ? cardLabel(card) : "Hidden";
    if (selectable) {
      el.addEventListener("click", () => {
      if (selectedCards.has(card)) {
        selectedCards.delete(card);
      } else {
        selectedCards.add(card);
      }
      updateReplaceButton();
      renderCards(target, cards, options);
      });
    }
    target.appendChild(el);
    });
  }

  function showdownId(state) {
    if (!state.showdown) return "";
    const playerIds = (state.showdown.players || []).map(player => player.id).join("-");
    return `${state.showdown.winner}:${playerIds}:${state.game?.round ?? 0}`;
  }

  function clearShowdownPromptTimer() {
    if (showdownPromptTimer) {
    clearTimeout(showdownPromptTimer);
    showdownPromptTimer = null;
    }
  }

  function scheduleShowdownPrompt(state) {
    const key = showdownId(state);
    if (!key || key === showdownKey) return;

    showdownKey = key;
    clearShowdownPromptTimer();

    const playerCount = state.showdown.players?.length || 0;
    showdownPromptTimer = setTimeout(showGameOverPrompt, 3000 + (500 * playerCount));
  }

  function updateReplaceButton() {
    document.querySelector("#replaceCards").disabled = selectedCards.size === 0;
  }

  function isReplacePhase(state) {
    return game.value === "FiveCardDraw" && state.game?.round === 1;
  }

  function updateActions(state) {
    const isMyTurn = joined && state.me && state.turn === state.me.id;
    const canAct = Boolean(isMyTurn && state.is_running);
    const replacePhase = canAct && isReplacePhase(state);
    const betPhase = canAct && !replacePhase;

    document.querySelector("#waitingHint").classList.toggle("is-hidden", canAct);
    document.querySelector("#betActions").classList.toggle("is-hidden", !betPhase);
    document.querySelector("#replaceActions").classList.toggle("is-hidden", !replacePhase);
    if (!betPhase) clearRaise();
  }

  function showStart() {
    document.querySelector("#startView").classList.remove("is-hidden");
    document.querySelector("#gameView").classList.add("is-hidden");
  }

  function showTable() {
    document.querySelector("#startView").classList.add("is-hidden");
    document.querySelector("#gameView").classList.remove("is-hidden");
    document.querySelector("#gameTitle").textContent = game.value;
  }

  function showGameOverPrompt() {
    gameOverPromptShown = true;
    document.querySelector("#gameOverModal").classList.remove("is-hidden");
  }

  function hideGameOverPrompt() {
    document.querySelector("#gameOverModal").classList.add("is-hidden");
  }

  function renderBoard(state) {
    const board = document.querySelector("#board");
    const cards = state.game?.community_cards || [];
    if (cards.length) {
    const winner = state.showdown?.players?.find(player => player.id === state.showdown?.winner);
    renderCards(board, cards, { rankCards: new Set(winner?.rank_cards || []) });
    return;
    }

    board.innerHTML = "";
    const empty = document.createElement("span");
    empty.className = "empty-board";
    empty.textContent = "No table cards";
    board.appendChild(empty);
  }

  function render(state) {
    renderPot(state.pot ?? 0);
    document.querySelector("#turn").textContent = state.turn ?? "-";
    document.querySelector("#round").textContent = state.game?.round ?? "-";
    const currentHand = new Set(state.me?.hand ?? []);
    selectedCards = new Set([...selectedCards].filter(card => currentHand.has(card)));
    updateReplaceButton();
    updateActions(state);
    renderBoard(state);

    if (joined && state.is_running) {
    hasSeenRunning = true;
    gameOverPromptShown = false;
    showdownKey = "";
    clearShowdownPromptTimer();
    hideGameOverPrompt();
    } else if (joined && state.me && hasSeenRunning && state.showdown) {
    scheduleShowdownPrompt(state);
    } else if (joined && state.me && hasSeenRunning && !gameOverPromptShown) {
    showGameOverPrompt();
    }

    const players = document.querySelector("#players");
    players.innerHTML = "";
    (state.players || []).forEach(player => {
    const isSelf = state.me?.id === player.id;
    const showdownPlayer = state.showdown?.players?.find(showdownPlayer => showdownPlayer.id === player.id);
    const showdownCards = showdownPlayer?.hand || null;
    const visibleCards = showdownCards || (isSelf ? state.me?.hand || [] : player.hand || []);
    const rankCards = new Set(showdownPlayer?.rank_cards || []);
    const communityCards = state.game?.community_cards || [];
    const usedCommunityCards = (showdownPlayer?.rank_cards || []).filter(card => communityCards.includes(card));
    const el = document.createElement("article");
    el.className = `player ${player.id === state.turn ? "current" : ""} ${isSelf ? "self" : ""} ${state.showdown?.winner === player.id ? "winner" : ""}`;
    el.innerHTML = `<h3>${escapeHtml(player.name)} ${isSelf ? '<span class="me-tag">(ME)</span>' : ''}</h3>
      <div class="chip-row wallet-chips"></div>
      <div class="chip-row bet-chips"></div>
      <div>${player.folded ? "Folded" : "Active"}</div>
      <div class="cards"></div>
      <div class="rank-label">${showdownPlayer?.rank || ""}</div>
      <div class="cards rank-used"></div>`;
    renderMoney(el.querySelector(".wallet-chips"), "Wallet", player.wallet);
    renderMoney(el.querySelector(".bet-chips"), "Bet", player.bet);
    renderCards(el.querySelector(".cards"), visibleCards, {
      selectable: !state.showdown && isSelf && isReplacePhase(state) && state.turn === state.me?.id,
      rankCards,
    });
    if (usedCommunityCards.length) {
      renderCards(el.querySelector(".rank-used"), usedCommunityCards, { rankCards });
    }
    players.appendChild(el);
    });
  }

  async function refresh() {
    if (!joined || !game.value) return;
    const id = joined ? playerId.value : "";
    const body = await api(`/api/games/${game.value}/state${id ? `?player_id=${id}` : ""}`);
    window.latestState = body.state;
    render(body.state);
    connection.textContent = joined ? `Joined ${game.value}` : "Connected";
  }

  async function requestRefresh() {
    if (refreshInFlight) {
    refreshQueued = true;
    return;
    }

    refreshInFlight = true;
    try {
    await refresh();
    } finally {
    refreshInFlight = false;
    }

    if (refreshQueued) {
    refreshQueued = false;
    requestRefresh();
    }
  }

  function connectSocket() {
    if (socket && (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING)) return;

    const protocol = location.protocol === "https:" ? "wss" : "ws";
    socket = new WebSocket(`${protocol}://${location.host}/ws`);
    socket.addEventListener("open", () => {
    connection.textContent = "Connected";
    requestRefresh().catch(() => {});
    });
    socket.addEventListener("message", () => requestRefresh().catch(() => {}));
    socket.addEventListener("close", () => {
    connection.textContent = "Disconnected";
    setTimeout(connectSocket, 1000);
    });
    socket.addEventListener("error", () => socket.close());
  }

  async function loadGames() {
    const body = await api("/api/games");
    game.innerHTML = "";
    body.games.forEach(name => {
    const option = document.createElement("option");
    option.value = name;
    option.textContent = name;
    game.appendChild(option);
    });
  }

  async function join() {
    localStorage.setItem("pokerClientName", nameInput.value);
    const body = await api(`/api/games/${game.value}/join`, {
    method: "POST",
    body: JSON.stringify({ name: nameInput.value, wallet: 1000 }),
    });
    joined = true;
    playerId.value = body.player_id;
    hasSeenRunning = false;
    gameOverPromptShown = false;
    hideGameOverPrompt();
    showTable();
    window.latestState = body.state;
    render(body.state);
  }

  async function send(command) {
    const body = await api(`/api/games/${game.value}/players/${playerId.value}/action`, {
    method: "POST",
    body: JSON.stringify({ command }),
    });
    window.latestState = body.state;
    render(body.state);
  }

  async function replaceSelected() {
    if (!selectedCards.size) return;
    const command = `REPLACE_CARDS ${[...selectedCards].join(" ")}`;
    selectedCards.clear();
    updateReplaceButton();
    await send(command);
  }

  async function raisePending() {
    if (pendingRaise <= 0) return;
    const amount = pendingRaise;
    clearRaise();
    await send(`RAISE ${amount}`);
  }

  async function standPat() {
    selectedCards.clear();
    updateReplaceButton();
    await send("REPLACE_CARDS");
  }

  async function leaveTable() {
    hideGameOverPrompt();
    if (joined) {
    try {
      await send("LEAVE");
    } catch (_) {}
    }

    joined = false;
    hasSeenRunning = false;
    gameOverPromptShown = false;
    selectedCards.clear();
    clearRaise();
    clearShowdownPromptTimer();
    showdownKey = "";
    updateReplaceButton();
    updateActions({ is_running: false, turn: null, me: null, game: {} });
    connection.textContent = "Connected";
    showStart();
  }

  async function keepPlaying() {
    hideGameOverPrompt();
    hasSeenRunning = false;
    gameOverPromptShown = true;
    await send("KEEP_PLAYING");
    requestRefresh().catch(() => {});
  }

  document.querySelector("#join").addEventListener("click", () => join().catch(alert));
  document.querySelector("#check").addEventListener("click", () => send("CHECK").catch(alert));
  document.querySelector("#fold").addEventListener("click", () => send("FOLD").catch(alert));
  document.querySelectorAll("[data-chip]").forEach(button => {
    button.addEventListener("click", () => addRaise(Number(button.dataset.chip)));
  });
  document.querySelector("#clearRaise").addEventListener("click", clearRaise);
  document.querySelector("#raise").addEventListener("click", () => raisePending().catch(alert));
  document.querySelector("#replaceCards").addEventListener("click", () => replaceSelected().catch(alert));
  document.querySelector("#standPat").addEventListener("click", () => standPat().catch(alert));
  document.querySelector("#leaveTable").addEventListener("click", () => leaveTable().catch(alert));
  document.querySelector("#leaveAfterGame").addEventListener("click", () => leaveTable().catch(alert));
  document.querySelector("#keepPlaying").addEventListener("click", () => keepPlaying().catch(alert));
  game.addEventListener("change", () => {
    document.querySelector("#gameTitle").textContent = game.value || "Table";
  });
  window.addEventListener("beforeunload", () => {
    if (!joined || !game.value || !playerId.value) return;
    fetch(`/api/games/${game.value}/players/${playerId.value}/action`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ command: "LEAVE" }),
    keepalive: true,
    });
  });
  setInterval(() => requestRefresh().catch(() => connection.textContent = "Disconnected"), 10000);
  ensureClientName();
  updateRaisePreview();
  connectSocket();
  loadGames().catch(alert);
  </script>
</body>
</html>"##;
