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

pub async fn serve(db: Arc<DbRepo>, addr: SocketAddr) {
  let state = WebState::new(db);
  let app = Router::new()
    .route("/favicon.svg", get(favicon))
    .route("/", get(index))
    .route("/app.css", get(css))
    .route("/app.js", get(js))
    .route("/ws", get(websocket))
    .route("/api/games", get(list_games))
    .route("/api/games/:game/state", get(game_state))
    .route("/api/games/:game/join", post(join_game))
    .route(
      "/api/games/:game/players/:player_id/action",
      post(player_action),
    )
    .with_state(state);

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

async fn favicon() -> impl IntoResponse {
  (
    [(axum::http::header::CONTENT_TYPE, "image/svg+xml")],
    include_str!("web/poker.svg"),
  )
}

async fn index() -> Html<&'static str> {
  Html(include_str!("web/index.html"))
}

async fn css() -> impl IntoResponse {
  (
    [(axum::http::header::CONTENT_TYPE, "text/css; charset=utf-8")],
    include_str!("web/app.css"),
  )
}

async fn js() -> impl IntoResponse {
  (
    [(axum::http::header::CONTENT_TYPE, "application/javascript; charset=utf-8")],
    include_str!("web/app.js"),
  )
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
