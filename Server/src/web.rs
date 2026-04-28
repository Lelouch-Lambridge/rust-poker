use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
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

type SharedTable<G> = Arc<Mutex<Table<G, DbRepo>>>;

#[derive(Clone)]
pub struct WebState {
    tables: Arc<HashMap<String, WebTable>>,
}

#[derive(Clone)]
enum WebTable {
    TexasHoldem(SharedTable<TexasHoldem>),
    FiveCardDraw(SharedTable<FiveCardDraw>),
    SevenCardStud(SharedTable<SevenCardStud>),
}

#[derive(Deserialize)]
struct JoinRequest {
    id: u64,
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

        tables.insert(
            "TexasHoldem".to_string(),
            WebTable::TexasHoldem(new_web_table::<TexasHoldem>("224.0.1.1:11000", db.clone())),
        );
        tables.insert(
            "FiveCardDraw".to_string(),
            WebTable::FiveCardDraw(new_web_table::<FiveCardDraw>("224.0.1.2:11001", db.clone())),
        );
        tables.insert(
            "SevenCardStud".to_string(),
            WebTable::SevenCardStud(new_web_table::<SevenCardStud>("224.0.1.3:11002", db)),
        );

        Self {
            tables: Arc::new(tables),
        }
    }

    fn table(&self, game: &str) -> Option<&WebTable> {
        self.tables.get(game)
    }
}

fn new_web_table<G: Game + Send + 'static>(udp_addr: &str, db: Arc<DbRepo>) -> SharedTable<G> {
    let table = Arc::new(Mutex::new(Table::<G, DbRepo>::new(udp_addr, db)));
    let monitor_table = table.clone();

    thread::spawn(move || loop {
        if {
            let table = monitor_table.lock().unwrap();
            table.get_num_players() >= 2 && !table.is_running()
        } {
            thread::sleep(Duration::from_secs(3));
            let mut table = monitor_table.lock().unwrap();
            if table.get_num_players() >= 2 && !table.is_running() {
                table.start_game();
            }
        }

        thread::sleep(Duration::from_secs(1));
    });

    table
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn list_games(State(state): State<WebState>) -> Json<Value> {
    let mut games = state.tables.keys().cloned().collect::<Vec<_>>();
    games.sort();
    Json(json!({ "games": games }))
}

async fn game_state(
    State(state): State<WebState>,
    Path(game): Path<String>,
    Query(query): Query<StateQuery>,
) -> impl IntoResponse {
    let Some(table) = state.table(&game) else {
        return json_error(StatusCode::NOT_FOUND, "Game not found");
    };

    Json(json!({
      "ok": true,
      "state": table.state(query.player_id),
    }))
    .into_response()
}

async fn join_game(
    State(state): State<WebState>,
    Path(game): Path<String>,
    Json(req): Json<JoinRequest>,
) -> impl IntoResponse {
    let Some(table) = state.table(&game) else {
        return json_error(StatusCode::NOT_FOUND, "Game not found");
    };

    if req.name.trim().is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "Name is required");
    }

    let player_id = req.id;

    match table.join(req).await {
        Ok(message) => Json(json!({
          "ok": true,
          "message": message,
          "state": table.state(Some(player_id)),
        }))
        .into_response(),
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
        Ok(message) => Json(json!({
          "ok": true,
          "message": message,
          "state": table.state(Some(player_id)),
        }))
        .into_response(),
        Err(message) => json_error(StatusCode::BAD_REQUEST, &message),
    }
}

impl WebTable {
    async fn join(&self, req: JoinRequest) -> Result<String, String> {
        match self {
            WebTable::TexasHoldem(table) => join_table(table, req).await,
            WebTable::FiveCardDraw(table) => join_table(table, req).await,
            WebTable::SevenCardStud(table) => join_table(table, req).await,
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
    req: JoinRequest,
) -> Result<String, String> {
    let db = table.lock().unwrap().db.clone();
    let player = if let Some(mut db_player) = db.fetch_player(req.id).await {
        db_player.name = req.name;
        db.upsert_player(&db_player).await;
        db_player
    } else {
        let new_player = Player {
            id: req.id,
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
    :root { color-scheme: light; font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
    body { margin: 0; background: #f6f7f9; color: #20242a; }
    header { display: flex; align-items: center; justify-content: space-between; gap: 16px; padding: 18px 24px; background: #ffffff; border-bottom: 1px solid #d9dee7; }
    h1 { margin: 0; font-size: 22px; font-weight: 700; letter-spacing: 0; }
    main { display: grid; grid-template-columns: 300px 1fr; gap: 24px; padding: 24px; max-width: 1180px; margin: 0 auto; }
    section, aside { min-width: 0; }
    .panel { background: #ffffff; border: 1px solid #d9dee7; border-radius: 8px; padding: 16px; }
    .stack { display: grid; gap: 12px; }
    label { display: grid; gap: 6px; font-size: 13px; font-weight: 600; }
    input, select { height: 38px; border: 1px solid #b8c0cc; border-radius: 6px; padding: 0 10px; font: inherit; background: #ffffff; }
    input[readonly] { color: #596574; background: #edf1f5; }
    button { height: 38px; border: 0; border-radius: 6px; padding: 0 12px; background: #245f73; color: white; font: inherit; font-weight: 700; cursor: pointer; }
    button.secondary { background: #596574; }
    button.danger { background: #a63f3f; }
    button:disabled { opacity: .5; cursor: default; }
    .row { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
    .table { display: grid; gap: 16px; }
    .summary { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 12px; }
    .metric { background: #edf3f5; border-radius: 8px; padding: 12px; min-height: 58px; }
    .metric span { display: block; font-size: 12px; color: #596574; }
    .metric strong { display: block; margin-top: 4px; font-size: 18px; }
    .players { display: grid; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr)); gap: 12px; }
    .player { border: 1px solid #d9dee7; border-radius: 8px; padding: 12px; background: #ffffff; }
    .player.current { border-color: #245f73; box-shadow: inset 0 0 0 1px #245f73; }
    .player h3 { margin: 0 0 8px; font-size: 15px; }
    .players .cards { margin-top: 10px; }
    .cards { display: flex; gap: 6px; flex-wrap: wrap; min-height: 44px; }
    .card { width: 34px; height: 44px; border: 1px solid #aeb7c4; border-radius: 6px; display: inline-grid; place-items: center; background: #fff; color: #20242a; font-size: 15px; font-weight: 800; line-height: 1; box-shadow: 0 1px 1px rgba(32,36,42,.08); }
    .card.red { color: #b72f3a; border-color: #e1a6ad; }
    .card.hidden { color: transparent; background: repeating-linear-gradient(45deg, #245f73 0 5px, #1e5061 5px 10px); border-color: #1e5061; }
    pre { white-space: pre-wrap; overflow-wrap: anywhere; margin: 0; max-height: 220px; overflow: auto; color: #39424e; }
    @media (max-width: 760px) { main { grid-template-columns: 1fr; padding: 16px; } header { padding: 14px 16px; } .summary { grid-template-columns: 1fr; } }
  </style>
</head>
<body>
  <header>
    <h1>Poker</h1>
    <div id="connection">Disconnected</div>
  </header>
  <main>
    <aside class="panel stack">
      <label>Game <select id="game"></select></label>
      <label>Player ID <input id="playerId" type="number" min="1" readonly></label>
      <label>Name <input id="name" value="Player"></label>
      <button id="join">Join Table</button>
      <div class="row">
        <button id="check" class="secondary">Check</button>
        <button id="fold" class="danger">Fold</button>
      </div>
      <div class="row">
        <input id="raiseAmount" type="number" min="1" value="50" style="flex:1">
        <button id="raise">Raise</button>
      </div>
      <label>Custom Command <input id="command" placeholder="REPLACE_CARDS 12 21"></label>
      <button id="sendCommand" class="secondary">Send Command</button>
    </aside>
    <section class="table">
      <div class="summary">
        <div class="metric"><span>Pot</span><strong id="pot">0</strong></div>
        <div class="metric"><span>Turn</span><strong id="turn">-</strong></div>
        <div class="metric"><span>Round</span><strong id="round">-</strong></div>
      </div>
      <div class="panel">
        <h2>Your Hand</h2>
        <div id="hand" class="cards"></div>
      </div>
      <div class="panel">
        <h2>Players</h2>
        <div id="players" class="players"></div>
      </div>
      <div class="panel">
        <h2>Table Data</h2>
        <pre id="raw">{}</pre>
      </div>
    </section>
  </main>
  <script>
    const game = document.querySelector("#game");
    const playerId = document.querySelector("#playerId");
    const nameInput = document.querySelector("#name");
    const connection = document.querySelector("#connection");
    let joined = false;

    function ensureClientId() {
      let id = localStorage.getItem("pokerClientId");
      if (!id) {
        const buffer = new Uint32Array(1);
        crypto.getRandomValues(buffer);
        id = String((buffer[0] % 900000000) + 100000000);
        localStorage.setItem("pokerClientId", id);
      }
      playerId.value = id;

      if (!localStorage.getItem("pokerClientName")) {
        localStorage.setItem("pokerClientName", `Player ${id.slice(-4)}`);
      }
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

    function renderCards(target, cards = []) {
      target.innerHTML = "";
      cards.forEach(card => {
        const el = document.createElement("span");
        el.className = cardClass(card);
        el.textContent = card ? cardLabel(card) : "Hidden";
        target.appendChild(el);
      });
    }

    function render(state) {
      document.querySelector("#pot").textContent = state.pot ?? 0;
      document.querySelector("#turn").textContent = state.turn ?? "-";
      document.querySelector("#round").textContent = state.game?.round ?? "-";
      renderCards(document.querySelector("#hand"), state.me?.hand ?? []);
      document.querySelector("#raw").textContent = JSON.stringify(state.game, null, 2);

      const players = document.querySelector("#players");
      players.innerHTML = "";
      (state.players || []).forEach(player => {
        const el = document.createElement("article");
        el.className = `player ${player.id === state.turn ? "current" : ""}`;
        el.innerHTML = `<h3>${player.name} #${player.id}</h3>
          <div>Wallet: ${player.wallet}</div>
          <div>Bet: ${player.bet}</div>
          <div>${player.folded ? "Folded" : "Active"}</div>
          <div class="cards"></div>`;
        renderCards(el.querySelector(".cards"), player.hand || []);
        players.appendChild(el);
      });
    }

    async function refresh() {
      if (!game.value) return;
      const id = joined ? playerId.value : "";
      const body = await api(`/api/games/${game.value}/state${id ? `?player_id=${id}` : ""}`);
      render(body.state);
      connection.textContent = joined ? `Joined ${game.value}` : "Connected";
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
      await refresh();
    }

    async function join() {
      localStorage.setItem("pokerClientName", nameInput.value);
      const body = await api(`/api/games/${game.value}/join`, {
        method: "POST",
        body: JSON.stringify({ id: Number(playerId.value), name: nameInput.value, wallet: 1000 }),
      });
      joined = true;
      render(body.state);
    }

    async function send(command) {
      const body = await api(`/api/games/${game.value}/players/${playerId.value}/action`, {
        method: "POST",
        body: JSON.stringify({ command }),
      });
      render(body.state);
    }

    document.querySelector("#join").addEventListener("click", () => join().catch(alert));
    document.querySelector("#check").addEventListener("click", () => send("CHECK").catch(alert));
    document.querySelector("#fold").addEventListener("click", () => send("FOLD").catch(alert));
    document.querySelector("#raise").addEventListener("click", () => send(`RAISE ${document.querySelector("#raiseAmount").value}`).catch(alert));
    document.querySelector("#sendCommand").addEventListener("click", () => send(document.querySelector("#command").value).catch(alert));
    game.addEventListener("change", () => refresh().catch(alert));
    setInterval(() => refresh().catch(() => connection.textContent = "Disconnected"), 1500);
    ensureClientId();
    loadGames().catch(alert);
  </script>
</body>
</html>"##;
