use bb8::Pool;
use bb8_postgres::PostgresConnectionManager;
use tokio_postgres::NoTls;
use postgres_types::Json;
use serde_json::Value;
use poker::{hand::Hand, player::Player};
use uuid::Uuid;
use log::{error, warn, info, debug, trace};

pub trait GameDatabase: Send + Sync {
  fn new(pool: Pool<PostgresConnectionManager<NoTls>>) -> Self where Self: Sized;

  fn save_game_state<'a>(&'a self, event: &'a str, game_type: &'a str, data: &'a Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;

  fn fetch_player<'a>(&'a self, id: u64) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<Player>> + Send + 'a>>;

  fn upsert_player<'a>(&'a self, player: &'a Player) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;
}

#[derive(Clone)]
pub struct DbRepo {
  pool: Pool<PostgresConnectionManager<NoTls>>,
}

impl DbRepo {
  pub async fn fetch_player(&self, id: u64) -> Option<Player> {
    info!("Fetching player with ID {}", id);
    let client = match self.pool.get().await {
      Ok(c) => c,
      Err(e) => {
        error!("Couldn't get DB connection: {}", e);
        return None;
      }
    };

    match client.query_opt("SELECT name, wallet FROM players WHERE id = $1", &[&(id as i64)]).await {
      Ok(Some(row)) => {
        let player = Player {
          id,
          name: row.get::<_, String>(0),
          wallet: row.get::<_, i64>(1) as u64,
          bet: 0,
          folded: false,
          hand: Hand(Vec::new()),
        };
        info!("Player found: ID={}, Name={}, Wallet={}", player.id, player.name, player.wallet);
        Some(player)
      }
      Ok(None) => {
        info!("No player found with ID {}", id);
        None
      }
      Err(e) => {
        error!("Failed to fetch player: {}", e);
        None
      }
    }
  }

  pub async fn upsert_player(&self, player: &Player) {
    info!("Upserting player: ID={}, Name={}, Wallet={}", player.id, player.name, player.wallet);

    let client = match self.pool.get().await {
      Ok(c) => c,
      Err(e) => {
        error!("Couldn't get DB connection: {}", e);
        return;
      }
    };

    let id = player.id as i64;
    let wallet = player.wallet as i64;

    match client.execute(
      "INSERT INTO players (id, name, wallet) VALUES ($1, $2, $3)
       ON CONFLICT(id) DO UPDATE SET name = $2, wallet = $3",
      &[&id, &player.name.as_str(), &wallet],
    ).await {
      Ok(n) => info!("Upsert success: {} row(s) affected", n),
      Err(e) => error!("Failed to upsert player: {}", e),
    };
  }

  pub async fn save_game_state(&self, event: &str, game_type: &str, data: &Value) {
    info!("Saving game state for event '{}', game_type '{}'\ndata: {}", event, game_type, data);
    let client = match self.pool.get().await {
      Ok(c) => c,
      Err(e) => {
        error!("Couldn't get DB connection: {}", e);
        return;
      }
    };

    let game_id = match data.get("game_id").and_then(|v| v.as_str()) {
      Some(gid) => match Uuid::parse_str(gid) {
        Ok(id) => id,
        Err(_) => {
          error!("Invalid game_id");
          return;
        }
      },
      None => {
        error!("Missing game_id");
        return;
      }
    };

    let round = data.get("round").and_then(|r| r.as_i64()).unwrap_or(0) as i32;

    let _ = client.execute(
      "INSERT INTO games (game_id, game_type) VALUES ($1, $2)
      ON CONFLICT DO NOTHING",
      &[&game_id, &game_type],
    ).await;

    let insert_result = client.query_one(
      "INSERT INTO game_states (game_id, round, event, data)
      VALUES ($1, $2, $3, $4)
      ON CONFLICT (game_id, round) DO NOTHING
      RETURNING round",
      &[&game_id, &round, &event, &Json(data.clone())],
    ).await;

    match insert_result {
      Ok(row) => {
        let saved_round: i32 = row.get(0);
        debug!("Inserted game_state: game_id={}, round={}", game_id, saved_round);
        let Some(players) = data.get("players").and_then(|p| p.as_array()) else { return; };

        for player in players {
          let Some(player_id) = player.get("id").and_then(|v| v.as_u64()) else { continue; };
          let _ = client.execute(
            "INSERT INTO game_players (game_id, player_id)
            VALUES ($1, $2)
            ON CONFLICT DO NOTHING",
            &[&game_id, &(player_id as i64)],
          ).await;
        }
      }
      Err(e) => {
        error!("Failed to insert {} game state: {}", event, e);
      }
    };

    if event != "end" { return; }

    let Some(winner_id) = data.get("winner").and_then(|v| v.as_u64()) else { return; };
    let _ = client.execute(
      "UPDATE games SET winner = $1 WHERE game_id = $2",
      &[&(winner_id as i64), &game_id],
    ).await;
  }
}

impl GameDatabase for DbRepo {
  fn new(pool: Pool<PostgresConnectionManager<NoTls>>) -> Self {
    trace!("Db Create");
    Self { pool }
  }

  fn save_game_state<'a>(&'a self, event: &'a str, game_type: &'a str, data: &'a Value,) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
      self.save_game_state(event, game_type, data).await;
    })
  }

  fn fetch_player<'a>(&'a self, id: u64) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<Player>> + Send + 'a>> {
    Box::pin(async move {
      self.fetch_player(id).await
    })
  }

  fn upsert_player<'a>(&'a self, player: &'a Player) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
      self.upsert_player(player).await
    })
  }
}
