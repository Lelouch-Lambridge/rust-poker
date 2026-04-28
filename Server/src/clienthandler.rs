use std::io::{BufRead, BufReader, Read, Write, ErrorKind};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use std::thread;
use log::{error, info, warn, debug};

use poker::{game::Game, hand::Hand, player::Player};
use server::{db_runtime, Table};

use server::db::GameDatabase;
use crate::servermanager::ServerManager;

fn ping_client(stream: &mut TcpStream) -> bool {
  match stream.write_all(b"PING\n") {
    Ok(_) => {
      stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
      let mut response = [0; 8];
      match stream.read(&mut response) {
        Ok(n) if n > 0 => {
          let resp = String::from_utf8_lossy(&response[..n]);
          resp.trim() == "PONG"
        },
        _ => false
      }
    },
    Err(_) => false
  }
}

fn handle_player_disconnect<G: Game, Db: GameDatabase + 'static>(player_id: u64, table: &Arc<Mutex<Table<G, Db>>>, is_turn: bool) {
  info!("Player {} disconnected, removing from table", player_id);
  
  let mut table_lock = table.lock().unwrap();
  
  table_lock.remove_player(player_id).unwrap_or_else(|e| {
    error!("Error removing player {}: {}", player_id, e);
    e
  });
  
  if is_turn { table_lock.advance_turn(); }
  
  table_lock.broadcast_state();
  
  if table_lock.players.len() < 2 {
    info!("Too few players remaining after disconnect, ending game");
    table_lock.end_game();
  }
}

// ========== Game Client Handling ========== 
pub fn handle_game_client<G: Game, Db: GameDatabase + 'static>(mut stream: TcpStream, table: Arc<Mutex<Table<G, Db>>>) {
  let mut buffer = [0; 1024];
  let mut player_id: Option<u64> = None;
  let mut last_ping_time = Instant::now();
  
  stream.set_read_timeout(Some(Duration::from_secs(1))).ok();

  loop {
    if let Ok(table_lock) = table.try_lock() { table_lock.broadcast_state(); }

    let is_turn = if let Some(id) = player_id {
      let table_lock = table.lock().unwrap();
      let turn_id = table_lock.turn.as_ref().map(|node| node.lock().unwrap().player.id);
      Some(id) == turn_id
    } else {
      false
    };
    
    if is_turn && player_id.is_some() && last_ping_time.elapsed() >= Duration::from_secs(30) {
      debug!("Pinging player {} who is currently taking their turn", player_id.unwrap());
      if !ping_client(&mut stream) {
        warn!("Player {} disconnected during their turn (ping failed)", player_id.unwrap());
        if let Some(id) = player_id {
          handle_player_disconnect(id, &table, is_turn);
        }
        return;
      }
      last_ping_time = Instant::now();
    }

    let bytes_read = match stream.read(&mut buffer) {
      Ok(0) => {
        if let Some(id) = player_id {
          warn!("Player {} disconnected gracefully", id);
          handle_player_disconnect(id, &table, is_turn);
        } else {
          info!("Unnamed client disconnected gracefully");
        }
        return;
      },
      Ok(n) => n,
      Err(e) => {
        match e.kind() {
          ErrorKind::WouldBlock | ErrorKind::TimedOut => {
            continue;
          },
          ErrorKind::BrokenPipe | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted => {
            if let Some(id) = player_id {
              warn!("Player {} connection error: {}", id, e);
              handle_player_disconnect(id, &table, is_turn);
            } else {
              warn!("Unnamed client connection error: {}", e);
            }
            return;
          },
          _ => {
            if let Some(id) = player_id {
              error!("Unexpected error reading from player {}: {}", id, e);
              handle_player_disconnect(id, &table, is_turn);
            } else {
              error!("Unexpected error reading from unnamed client: {}", e);
            }
            return;
          }
        }
      }
    };

    last_ping_time = Instant::now();

    let request: String = String::from_utf8_lossy(&buffer[..bytes_read]).to_string();
    let parts: Vec<&str> = request.trim().split_whitespace().collect();

    if parts.is_empty() {
      let _ = stream.write_all(b"INVALID COMMAND\n");
      continue;
    }

    if parts.as_slice() == ["PONG"] {
      debug!("Received PONG from player {:?}", player_id);
      continue;
    }

    if let Some(should_continue) = handle_debug_commands(&parts, &stream, &table) {
      if !should_continue {
        return;
      }
    }

    if let ["JOIN", id, name, wallet] = parts.as_slice() {
      match handle_join(&mut player_id, id, name, wallet, &stream, &table) {
        Ok(_) => continue,
        Err(e) => {
          let _ = stream.write_all(e.as_bytes());
          return;
        }
      }
    }

    let Some(id) = player_id else {
      let _ = stream.write_all(b"[ERROR] NOT JOINED\n");
      continue;
    };

    if let ["LEAVE"] = parts.as_slice() {
      handle_player_disconnect(id, &table, is_turn);
      let _ = stream.write_all(b"REMOVED\n");
      return;
    }

    if let ["UPDATE"] = parts.as_slice() {
      let table_lock = table.lock().unwrap();
      match table_lock.player_send(id) {
        Ok(msg) => info!("Ok: {}", msg),
        Err(msg) => error!("Err: {}", msg),
      }
      continue;
    }

    let turn_id = table.lock().unwrap().turn.as_ref().map(|node| node.lock().unwrap().player.id);
    if Some(id) != turn_id {
      let _ = stream.write_all(b"[ERROR] NOT YOUR TURN\n");
      continue;
    }

    {
      let mut table_lock = table.lock().unwrap();
      match table_lock.game_actions(id, &parts) {
        Ok(msg) => {
          if let Err(e) = stream.write_all(msg.as_bytes()) {
            error!("Error writing to player {}: {}", id, e);
            handle_player_disconnect(id, &table, is_turn);
            return;
          }
          table_lock.advance_turn();
          continue; 
        }
        Err(msg) if msg == "[BET]" => {},
        Err(msg) => {
          if let Err(e) = stream.write_all(msg.as_bytes()) {
            error!("Error writing to player {}: {}", id, e);
            handle_player_disconnect(id, &table, is_turn);
            return;
          }
          continue;
        }
      }
    }

    match parts.as_slice() {
      ["RAISE", amount] => {
        if amount.parse::<u64>().is_err() {
          if let Err(e) = stream.write_all(b"[ERROR] INVALID AMOUNT\n") {
            error!("Error writing to player {}: {}", id, e);
            handle_player_disconnect(id, &table, is_turn);
            return;
          }
          continue;
        }

        let result = table.lock().unwrap().raise(id, amount.parse().unwrap());
        match result {
          Ok(msg) => {
            if let Err(e) = stream.write_all(msg.as_bytes()) {
              error!("Error writing to player {}: {}", id, e);
              handle_player_disconnect(id, &table, is_turn);
              return;
            }
          },
          Err(e) => {
            if let Err(write_err) = stream.write_all(e.as_bytes()) {
              error!("Error writing to player {}: {}", id, write_err);
              handle_player_disconnect(id, &table, is_turn);
              return;
            }
            continue;
          }
        }
      }
      ["CHECK"] => {
        let result = table.lock().unwrap().check(id);
        match result {
          Ok(msg) => {
            if let Err(e) = stream.write_all(msg.as_bytes()) {
              error!("Error writing to player {}: {}", id, e);
              handle_player_disconnect(id, &table, is_turn);
              return;
            }
          },
          Err(e) => {
            if let Err(write_err) = stream.write_all(e.as_bytes()) {
              error!("Error writing to player {}: {}", id, write_err);
              handle_player_disconnect(id, &table, is_turn);
              return;
            }
            continue;
          }
        }
      }
      ["FOLD"] => {
        let result = table.lock().unwrap().fold(id);
        match result {
          Ok(msg) => {
            if let Err(e) = stream.write_all(msg.as_bytes()) {
              error!("Error writing to player {}: {}", id, e);
              handle_player_disconnect(id, &table, is_turn);
              return;
            }
          },
          Err(e) => {
            if let Err(write_err) = stream.write_all(e.as_bytes()) {
              error!("Error writing to player {}: {}", id, write_err);
              handle_player_disconnect(id, &table, is_turn);
              return;
            }
            continue;
          }
        }
      }
      _ => {
        if let Err(e) = stream.write_all(b"[ERROR] INVALID COMMAND\n") {
          error!("Error writing to player {}: {}", id, e);
          handle_player_disconnect(id, &table, is_turn);
          return;
        }
        continue;
      }
    }
    table.lock().unwrap().advance_turn();
  }
}

// ========== Debug Commands ========== 
fn handle_debug_commands<G: Game, Db: GameDatabase + 'static>(parts: &[&str], mut stream: &TcpStream, table: &Arc<Mutex<Table<G, Db>>>) -> Option<bool> {
  match parts {
    ["DEBUG"] => {
      println!("{}", table.lock().unwrap());
      Some(false)
    }
    ["DEBUG_PING"] => {
      let _ = stream.write_all(b"PONG\n");
      Some(false)
    }
    ["DEBUG_UDP"] => {
      table.lock().unwrap().broadcast_state();
      let _ = stream.write_all(b"UDP SENT\n");
      Some(false)
    }
    ["DEBUG_DECK"] => {
      println!("Deck: {}", table.lock().unwrap().game.get_deck());
      Some(false)
    }
    ["DEBUG_CARDS"] => {
      println!("Players: [\n {}\n]",
        table.lock().unwrap().players.iter()
        .filter_map(|(id, node)| {
          let player = node.lock().unwrap();
          if !player.player.folded {
            Some(format!("{}: {}", id, player.player.hand))
          } else {
            None
          }
        }).collect::<Vec<String>>().join("\n "));
      println!("Table: [{}]", table.lock().unwrap().game);
      Some(false)
    }
    ["DEBUG_START"] => {
      table.lock().unwrap().start_game();
      Some(false)
    }
    ["DEBUG_SKIP"] => {
      table.lock().unwrap().advance_turn();
      Some(false)
    }
    ["DEBUG_END"] => {
      table.lock().unwrap().end_game();
      Some(false)
    }
    _ => None,
  }
}

// ========== Player Join Handling ==========
fn handle_join<G: Game, Db: GameDatabase + 'static>(player_id: &mut Option<u64>, id: &str, name: &str, _wallet: &str, mut stream: &TcpStream, table: &Arc<Mutex<Table<G, Db>>>) -> Result<(), String> {
  let parsed_id = id.parse::<u64>().map_err(|_| "[ERROR] INVALID ID VALUE\n".to_string())?;
  *player_id = Some(parsed_id);

  let db = table.lock().unwrap().db.clone();
  let player_name = name.to_string();

  let player = db_runtime().block_on(async {
    if let Some(mut db_player) = db.fetch_player(parsed_id).await {
      db_player.name = player_name.clone();
      db.upsert_player(&db_player).await;
      db_player
    } else {
      let new_player = Player {
        id: parsed_id,
        name: player_name.clone(),
        wallet: 1000,
        bet: 0,
        folded: false,
        hand: Hand(Vec::new()),
      };
      db.upsert_player(&new_player).await;
      new_player
    }
  });

  let cloned_stream = stream.try_clone().map_err(|e| format!("[ERROR] {}", e))?;
  table.lock().unwrap().add_player_with_stream(player, cloned_stream)?;
  let _ = stream.write_all(b"JOINED\n");
  Ok(())
}

// ========== Lobby Client Handling ==========
pub fn handle_lobby_client<Db: GameDatabase + 'static>(mut stream: TcpStream, server_manager: Arc<Mutex<ServerManager<Db>>>) {
  stream.set_read_timeout(Some(Duration::from_secs(1))).ok();
  
  let mut lobby_client_added = false;
  if let Ok(manager) = server_manager.lock() {
    let arc_clients = manager.get_lobby();
    let mut clients = arc_clients.lock().unwrap();
    if let Ok(clone) = stream.try_clone() {
      clients.push(Arc::new(Mutex::new(clone)));
      lobby_client_added = true;
    }
  }

  let message = b"Welcome to the poker lobby!\n";
  if let Err(e) = stream.write_all(message) {
    error!("Error writing to lobby client: {}", e);
    return;
  }

  if let Ok(manager) = server_manager.lock() {
    let games = manager.list_games();
    let game_list = format!("[GAMES] {{\"games\":[\"{}\"]}}\n", games.join("\",\""));
    if let Err(e) = stream.write_all(game_list.as_bytes()) {
      error!("Error writing game list to lobby client: {}", e);
      return;
    }
  }

  let mut reader = BufReader::new(stream.try_clone().unwrap());
  let mut last_ping_time = Instant::now();

  loop {
    if last_ping_time.elapsed() >= Duration::from_secs(60) {
      if !ping_client(&mut stream) {
        warn!("Lobby client disconnected (ping failed)");
        break;
      }
      last_ping_time = Instant::now();
    }

    let mut line = String::new();
    match reader.read_line(&mut line) {
      Ok(0) => {
        info!("[INFO] Client disconnected from lobby");
        break;
      }
      Ok(_) => {
        if line.trim() == "PONG" {
          debug!("Received PONG from lobby client");
          continue;
        }
        
        let game_name = line.trim();
        info!("[INFO] Lobby received game request: {}", game_name);

        if let Ok(manager) = server_manager.lock() {
          let arc_tables = manager.get_tables();
          let tables = arc_tables.lock().unwrap();
          if let Some(info) = tables.get(game_name) {
            let response = format!(
              "[GAME_INFO] {{\"port\":{},\"multicast\":\"{}\"}}\n",
              info.get_port(), info.get_multicast_addr()
            );
            if let Err(e) = stream.write_all(response.as_bytes()) {
              error!("Error writing game info to lobby client: {}", e);
              break;
            }
          } else {
            if let Err(e) = stream.write_all(b"[ERROR] Game not found\n") {
              error!("Error writing error to lobby client: {}", e);
              break;
            }
          }
        } else {
          if let Err(e) = stream.write_all(b"[ERROR] Server unavailable\n") {
            error!("Error writing error to lobby client: {}", e);
            break;
          }
        }
      }
      Err(e) => {
        if e.kind() != ErrorKind::WouldBlock && e.kind() != ErrorKind::TimedOut {
          error!("[ERROR] Lobby client read error: {}", e);
          break;
        }
        thread::sleep(Duration::from_millis(100));
      }
    }
  }
  
  if lobby_client_added {
    if let Ok(manager) = server_manager.lock() {
      let arc_clients = manager.get_lobby();
      let mut clients = arc_clients.lock().unwrap();
      let client_addr = stream.peer_addr().ok();
      if let Some(addr) = client_addr {
        clients.retain(|c| {
          if let Ok(c_lock) = c.lock() {
            if let Ok(c_addr) = c_lock.peer_addr() {
              return c_addr != addr;
            }
          }
          true
        });
      }
    }
  }
}
