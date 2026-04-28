use std::fmt;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::net::{TcpStream, UdpSocket, SocketAddr, IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex, OnceLock};
use socket2::{Socket, Domain, Type, Protocol, SockRef};
use serde_json::{json, to_string};
use poker::{hand::Hand, player::{Player, PlayerNode}, game::Game};
use log::{error, warn, info, debug, trace};
use std::any::type_name;
pub mod db;
use db::GameDatabase;
use uuid::Uuid;

pub fn db_runtime() -> &'static tokio::runtime::Runtime {
  static DB_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
  DB_RUNTIME.get_or_init(|| tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime"))
}

pub struct Table<G: Game, Db: GameDatabase> {
  game_id: Uuid,
  pub players: HashMap<u64, Arc<Mutex<PlayerNode>>>, // Fast lookup by ID
  pub head: Option<Arc<Mutex<PlayerNode>>>,
  pub bettor: Option<Arc<Mutex<PlayerNode>>>,
  pub blind: Option<Arc<Mutex<PlayerNode>>>,
  pub turn: Option<Arc<Mutex<PlayerNode>>>,
  pub bet: u64,
  pub pot: u64,
  pub udp_port: String,
  pub socket_udp: UdpSocket,
  pub game: G,
  pub db: Arc<Db>,
  pub showdown: Option<serde_json::Value>,
  pub next_game_players: HashSet<u64>,
}

fn get_local_ip() -> Ipv4Addr {
  if let Ok(IpAddr::V4(ip)) = local_ip_address::local_ip() {
    return ip;
  }

  let socket = match UdpSocket::bind("0.0.0.0:0") {
    Ok(socket) => socket,
    Err(e) => {
      warn!("Failed to bind dummy socket for local IP detection: {}", e);
      return Ipv4Addr::LOCALHOST;
    }
  };

  if socket.connect("8.8.8.8:80").is_ok() {
    if let Ok(SocketAddr::V4(addr)) = socket.local_addr() {
      return *addr.ip();
    }
  }

  warn!("Unable to detect routed IPv4 address; falling back to loopback");
  Ipv4Addr::LOCALHOST
}

impl<G: Game, Db: GameDatabase + 'static> Table<G, Db> {
  pub fn new(udp_port: &str, db: Arc<Db>) -> Table<G, Db> {
    use std::net::Ipv4Addr;

    let multicast_addr: SocketAddr = udp_port.parse().expect("Invalid UDP address");
    let multicast_ip = match multicast_addr.ip() {
      IpAddr::V4(ip) => ip,
      _ => panic!("Only IPv4 multicast supported"),
    };
    let port = multicast_addr.port();
    let local_ip = get_local_ip();

    info!("Joining multicast group {} on local IP {}", multicast_ip, local_ip);

    let recv_socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))
      .expect("Failed to create receiver UDP socket");
    recv_socket.set_reuse_address(true).expect("SO_REUSEADDR failed");
    
    #[cfg(unix)]
    {
      SockRef::from(&recv_socket).set_reuse_port(true).expect("SO_REUSEPORT failed");
    }
    let bind_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), port);
    recv_socket.bind(&bind_addr.into()).expect("Bind failed");

    let socket_udp_recv = UdpSocket::from(recv_socket);
    socket_udp_recv
      .join_multicast_v4(&multicast_ip, &local_ip)
      .expect("Join multicast group failed");

    let socket_udp_send = UdpSocket::bind("0.0.0.0:0").expect("Failed to bind sender socket");
    socket_udp_send
      .set_multicast_loop_v4(true)
      .expect("Failed to enable loopback");
    SockRef::from(&socket_udp_send)
      .set_multicast_if_v4(&local_ip)
      .expect("Failed to set multicast iface");

    Table {
      game_id: Uuid::new_v4(),
      players: HashMap::new(), 
      head: None, bettor: None, blind: None, turn: None,
      bet: 0, pot: 0,
      socket_udp: socket_udp_send, udp_port: udp_port.to_string(),
      game: G::new(), db, showdown: None, next_game_players: HashSet::new(),
    }
  }

  pub fn get_num_players(&self) -> usize {
    self.players.len()
  }

  pub fn is_running(&self) -> bool {
    self.turn.is_some()
  }

  fn rebuild_full_ring(&mut self) {
    if self.players.is_empty() {
      self.head = None;
      self.blind = None;
      return;
    }

    let nodes = self.players.values().cloned().collect::<Vec<_>>();
    let len = nodes.len();

    for (index, node) in nodes.iter().enumerate() {
      let prev = nodes[(index + len - 1) % len].clone();
      let next = nodes[(index + 1) % len].clone();
      let mut locked = node.lock().unwrap();
      locked.prevf = Some(prev);
      locked.nextf = Some(next);
    }

    let head_still_present = self.head.as_ref().is_some_and(|head| {
      self.players.values().any(|node| Arc::ptr_eq(node, head))
    });
    if !head_still_present {
      self.head = Some(nodes[0].clone());
    }

    let blind_still_present = self.blind.as_ref().is_some_and(|blind| {
      self.players.values().any(|node| Arc::ptr_eq(node, blind))
    });
    if !blind_still_present {
      self.blind = self.head.clone();
    }
  }

  fn reset(&mut self) {
    self.bet = 0;
    self.pot = 0;
  }

  pub fn add_player(&mut self, player: Player) -> Result<String, String> {
    self.add_player_with_optional_stream(player, None)
  }

  pub fn add_player_with_stream(&mut self, player: Player, stream: TcpStream) -> Result<String, String> {
    self.add_player_with_optional_stream(player, Some(stream))
  }

  fn add_player_with_optional_stream(&mut self, player: Player, stream: Option<TcpStream>) -> Result<String, String> {
    trace!("Adding player…");
    let id = player.id;
    if self.players.contains_key(&player.id) { return Err(format!("Player with ID {} already exists", player.id)); }
    
    let new_node = Arc::new(Mutex::new(PlayerNode { player, stream, nextf: None, prevf: None, next: None, prev: None, }));
    
    match &self.head {
      Some(existing_head) => {
        let existing_tail = existing_head.lock().unwrap().prevf.clone().unwrap();
        existing_tail.lock().unwrap().nextf = Some(new_node.clone());
        new_node.lock().unwrap().prevf = Some(existing_tail.clone());
        
        let head = self.head.as_ref().unwrap().clone();
        new_node.lock().unwrap().nextf = Some(head.clone());
        head.lock().unwrap().prevf = Some(new_node.clone());
      }
      None => {
        self.head = Some(new_node.clone());
        self.blind = Some(new_node.clone());
        new_node.lock().unwrap().nextf = Some(new_node.clone());
        new_node.lock().unwrap().prevf = Some(new_node.clone());
      }
    }
    
    self.players.insert(new_node.lock().unwrap().player.id, new_node.clone());
    self.player_send(id)
  }
  
  pub fn get_player(&self, player_id: u64) -> Option<Arc<Mutex<PlayerNode>>> {
    self.players.get(&player_id).cloned()
  }

  pub fn raise(&mut self, player_id: u64, amount: u64) -> Result<String, String> {
    trace!("Raising…");
    let node = self.players.get(&player_id).ok_or("Player not found")?;
    self.bettor = Some(node.clone());
    let mut player_locked = node.lock().map_err(|_| "Lock failed")?;

    if player_locked.player.wallet < amount { return Err("Insufficient funds".into()); }

    player_locked.player.wallet -= amount;
    player_locked.player.bet += amount;
    self.pot += amount;
    self.bet = player_locked.player.bet;

    let player = player_locked.player.clone();
    drop(player_locked);
    self.persist_player(player);
    self.player_send(player_id)
  }
  
  pub fn check(&mut self, player_id: u64) -> Result<String, String> {
    trace!("Checking…");
    let node = self.players.get(&player_id).ok_or("Player not found")?;
    let mut player_locked = node.lock().map_err(|_| "Lock failed")?;

    if player_locked.player.bet == self.bet {
      drop(player_locked);
      return self.player_send(player_id);
    }

    let diff = self.bet - player_locked.player.bet;
    if player_locked.player.wallet < diff { return Err("Insufficient funds to check".into()); }

    self.pot += diff;
    player_locked.player.bet = self.bet;
    player_locked.player.wallet -= diff;

    let player = player_locked.player.clone();
    drop(player_locked);
    self.persist_player(player);
    self.player_send(player_id)
  }

  // TODO Fix pointers reassignmnets for folding and leaving
  pub fn fold(&mut self, player_id: u64) -> Result<String, String> {
    trace!("Folding…");
    let node = self.players.get(&player_id).ok_or("Player not found")?;
    let mut player_locked = node.lock().map_err(|_| "Lock failed")?;

    player_locked.player.folded = true;

    let prev_active = player_locked.prev.clone();
    let next_active = player_locked.next.clone();

    player_locked.prev = None;
    player_locked.next = None;

    if let Some(prev) = &prev_active { prev.lock().map_err(|_| "Lock failed")?.next = next_active.clone(); }
    if let Some(next) = &next_active { next.lock().map_err(|_| "Lock failed")?.prev = prev_active.clone(); }

    let player = player_locked.player.clone();
    drop(player_locked);

    if let Some(head_node) = &self.head {
      if Arc::ptr_eq(head_node, &node) { self.head = next_active.clone(); }
    }

    if let Some(bettor_node) = &self.bettor {
      if Arc::ptr_eq(bettor_node, &node) { self.bettor = prev_active.clone(); }
    }

    if let Some(blind_node) = &self.blind {
      if Arc::ptr_eq(blind_node, &node) { self.blind = prev_active.clone(); }
    }

    if let Some(turn_node) = &self.turn {
      if Arc::ptr_eq(turn_node, &node) { self.turn = prev_active.clone(); }
    }

    if let (Some(prev), Some(next)) = (prev_active, next_active) {
      if Arc::ptr_eq(&prev, &next) { self.end_game(); }
    }

    self.persist_player(player);
    self.player_send(player_id)
  }

  pub fn remove_player(&mut self, player_id: u64) -> Result<String, String> {
    trace!("Removing player…");
    self.fold(player_id)?;

    let node = self.players.get(&player_id).cloned().ok_or("Player not found")?;

    let (prevf, nextf) = {
      let mut node_lock = node.lock().map_err(|_| "Lock failed")?;
      (node_lock.prevf.take(), node_lock.nextf.take())
    };

    if let Some(prevf_node) = &prevf { prevf_node.lock().map_err(|_| "Lock failed")?.nextf = nextf.clone(); }
    if let Some(nextf_node) = &nextf { nextf_node.lock().map_err(|_| "Lock failed")?.prevf = prevf.clone(); }

    if let Some(head) = &self.head {
      if Arc::ptr_eq(head, &node) {
        self.head = if nextf.as_ref().map_or(false, |n| Arc::ptr_eq(n, head)) {
          None
        } else {
          nextf.clone()
        };
      }
    }

    self.players.remove(&player_id).ok_or("Player not found")?;
    self.rebuild_full_ring();
    self.delete_player(player_id);
    Ok("REMOVED\n".to_string())
  }

  pub fn game_actions(&mut self, player_id: u64, parts: &[&str]) -> Result<String, String>{
    trace!("Game actions…");
    let node = self.players.get_mut(&player_id).ok_or("Player not found")?;
    let result = self.game.client_action(node, &parts);
    if let Err(e) = result { return Err(e.to_string()); }
    self.player_send(player_id)
  }

  pub fn start_game(&mut self) {
    trace!("Starting Game…");
    self.rebuild_full_ring();
    self.game_id = Uuid::new_v4();
    self.showdown = None;
    self.next_game_players.clear();
    let head = match self.head.as_ref() {
      Some(node) => node.clone(),
      None => {
        error!("No players to start game");
        return;
      }
    };

    let mut current = Some(head.clone());
    let mut first_iteration = true;

    while let Some(player_node) = current.take() {
      let mut player_locked = player_node.lock().unwrap();

      player_locked.player.folded = false;
      player_locked.player.hand = Hand(Vec::new());

      player_locked.next = player_locked.nextf.clone();
      player_locked.prev = player_locked.prevf.clone();

      current = player_locked.nextf.clone();

      if let Some(next_node) = &current {
        if Arc::ptr_eq(next_node, &head) && !first_iteration { break; }
      }
      first_iteration = false;
    }

    self.reset();
    self.game.start_game();
    
    let Some(blind_node) = self.blind.clone() else {
      error!("No blind to start game");
      return;
    };
    
    let blind_id = blind_node.lock().unwrap().player.id;
    let _ = self.raise(blind_id, 50);

    if self.game.is_ante() {
      self.turn = Some(blind_node.clone());

      let mut current = Some(blind_node.clone());
      let mut first_iteration = true;

      while let Some(player_node) = current.take() {
        let player_locked = player_node.lock().unwrap();
        current = player_locked.nextf.clone();
        let player_id = player_locked.player.id;
        drop(player_locked);

        let _ = self.check(player_id);

        if let Some(next_node) = &current {
          if Arc::ptr_eq(next_node, &blind_node) && !first_iteration {
            break;
          }
        }
        first_iteration = false;
      }
    } else {
      self.turn = blind_node.lock().unwrap().next.clone();
    }

    self.round_action();
    self.players_updated();
    self.trigger_save("start");
    info!("Game started");
  }

  pub fn end_game(&mut self) {
    trace!("Ending game…");
    let players_hands: Vec<(u64, Hand)> = self.players.iter()
      .filter_map(|(id, node)| {
        let player = node.lock().ok()?;
        if !player.player.folded {
          Some((*id, player.player.hand.clone()))
        } else {
          None
        }
      })
      .collect();

    if players_hands.is_empty() {
      error!("Players none");
      return;
    }
    
    let winner = self.game.determine_winner(
      &players_hands.iter().map(|(id, hand)| (id, hand)).collect::<Vec<_>>()
    );
    let Some(winner_id) = winner else { return; };
    info!("{}", winner_id);

    let showdown_entries: Vec<_> = self.players.iter()
      .filter_map(|(id, node)| {
        let player = node.lock().ok()?.player.clone();
        if player.folded {
          return None;
        }

        let showdown_hand = self.game.showdown_hand(&player.hand);
        let rank = showdown_hand.evaluate();

        Some((*id, player, showdown_hand, rank))
      })
      .collect();
    let ranks = showdown_entries
      .iter()
      .map(|(_, _, _, rank)| rank.clone())
      .collect::<Vec<_>>();
    let showdown_players: Vec<_> = showdown_entries.into_iter()
      .map(|(id, player, showdown_hand, rank)| {
        let rank_cards = rank.cards_used_against(&showdown_hand.0, &ranks);
        json!({
          "id": id,
          "name": player.name,
          "hand": player.hand,
          "rank": rank.to_string(),
          "rank_cards": rank_cards,
        })
      })
      .collect();
    self.showdown = Some(json!({
      "winner": winner_id,
      "players": showdown_players,
    }));
    self.next_game_players.clear();
    
    let Some(winner_node) = self.players.get(&winner_id) else { return; };
    
    let mut winner_locked = winner_node.lock().unwrap();
    winner_locked.player.wallet += self.pot;
    drop(winner_locked);
    
    for (id, node) in self.players.iter() {
      let mut player_node = node.lock().unwrap();
      player_node.player.reset();
      player_node.next = None;
      player_node.prev = None;
      let player = player_node.player.clone();
      self.persist_player(player);
      drop(player_node);
      match self.player_send(*id) {
        Ok(msg) => info!("Ok: {}", msg),
        Err(msg) => warn!("Err: {}", msg),
      }
    }
    
    self.head = Some(winner_node.clone());
    if let Some(blind_node) = self.blind.take() {
      self.blind = blind_node.lock().unwrap().nextf.clone();
    }
    self.turn = None;
    self.bettor = None;
    self.pot = 0;

    let winner = winner_node.lock().unwrap().player.clone();
    info!("Winner: ID = {}, Name = {}, Wallet = {}, Hand = {}", winner.id, winner.name, winner.wallet, winner.hand);
    self.trigger_save("end");
  }

  pub fn advance_turn(&mut self) {
    trace!("Advancing turn…");
    let Some(current_node) = self.turn.clone() else { return; };
    let current_id = current_node.lock().unwrap().player.id;
    let next = current_node.lock().unwrap().next.clone();
    let Some(next_player) = next else { return; };
    
    self.turn = Some(next_player.clone());
    
    let is_round_complete = {
      let bettor_id = self.bettor.as_ref().map(|n| n.lock().unwrap().player.id);
      next_player.lock().unwrap().player.id == bettor_id.unwrap_or(0)
    };
    
    let all_matched = {
      let mut current = Some(current_node.clone());
      let mut first_player = true;
      let mut matched = true;
      
      while let Some(player_node) = current.take() {
        let player_locked = player_node.lock().unwrap();
        
        if !first_player && player_locked.player.bet != self.bet {
          matched = false;
          break;
        }
        
        current = player_locked.next.clone();
        first_player = false;
        
        if let Some(next) = &current {
          if next.lock().unwrap().player.id == current_id { break; }
        }
      }
      matched
    };
    
    if is_round_complete && all_matched {
      self.game.advance_round();
      if self.round_action() {
        self.trigger_save("advance");
      }
      self.players_updated();
    }
  }

  pub fn keep_playing(&mut self, player_id: u64) -> Result<String, String> {
    if self.showdown.is_none() {
      return Err("[ERROR] NO SHOWDOWN".to_string());
    }

    if !self.players.contains_key(&player_id) {
      return Err("Player not found".to_string());
    }

    self.next_game_players.insert(player_id);
    Ok("KEEP_PLAYING\n".to_string())
  }

  pub fn remove_unconfirmed_players(&mut self) {
    if self.showdown.is_none() {
      return;
    }

    let unconfirmed = self.players
      .keys()
      .copied()
      .filter(|id| !self.next_game_players.contains(id))
      .collect::<Vec<_>>();

    for id in unconfirmed {
      let _ = self.remove_player(id);
    }
  }

  fn round_action(&mut self) -> bool {
    trace!("Round action…");
    if self.game.handle_round_action(&mut self.turn) {
      self.end_game();
      return false;
    }
    true
  }

  fn persist_player(&self, player: Player) {
    let db = Arc::clone(&self.db);

    std::thread::spawn(move || {
      db_runtime().block_on(async move {
        db.upsert_player(&player).await;
      });
    });
  }

  fn delete_player(&self, player_id: u64) {
    let db = Arc::clone(&self.db);

    std::thread::spawn(move || {
      db_runtime().block_on(async move {
        db.delete_player(player_id).await;
      });
    });
  }

  pub fn trigger_save(&self, event: &str) {
    trace!("trigger_save called for event: {}", event);
    let snapshot = self.snapshot_state(event);
    let event = event.to_string();
    let game_type = snapshot["game_type"].as_str().unwrap_or("Unknown").to_string();
    let db = Arc::clone(&self.db);

    std::thread::spawn(move || {
      db_runtime().block_on(async move {
        debug!("Saving game state: {}", event);
        db.save_game_state(&event, &game_type, &snapshot).await;
        debug!("Finished saving game state: {}", event);
      });
    });
  }

  fn type_name_short<T: ?Sized>() -> String {
    let full_name = type_name::<T>();
    full_name.split("::").last().unwrap_or(full_name).to_string()
  }

  pub fn snapshot_state(&self, event: &str) -> serde_json::Value {
    let turn_id = self.turn.as_ref().map(|n| n.lock().unwrap().player.id);
    let game_type = Self::type_name_short::<G>();

    let players: Vec<_> = self.players
      .iter()
      .map(|(_, node)| {
        let p = node.lock().unwrap().player.clone();
        json!({
          "id": p.id,
          "name": p.name,
          "wallet": p.wallet,
          "folded": p.folded,
        })
      }).collect();

    let mut snapshot = json!({
      "event": event,
      "game_id": self.game_id.to_string(),
      "round": self.game.get_round(),
      "turn": turn_id,
      "pot": self.pot,
      "players": players,
      "game": self.game.to_broadcast(),
      "game_type": game_type,
    });

    if event != "end" { return snapshot; }
    let Some(winner_node) = self.head.as_ref() else { return snapshot; };
    let winner = winner_node.lock().unwrap().player.id;
    snapshot["winner"] = json!(winner);

    snapshot
  }

  pub fn player_view_state(&self, player_id: Option<u64>) -> serde_json::Value {
    let turn_id = self.turn.as_ref().map(|node| node.lock().unwrap().player.id);

    let players: Vec<serde_json::Value> = self.players.values()
      .map(|node| json!(node.lock().unwrap().player.clone().to_broadcast()))
      .collect();

    let me = player_id
      .and_then(|id| self.players.get(&id).cloned())
      .map(|node| json!(node.lock().unwrap().player.clone().to_stream()));

    json!({
      "turn": turn_id,
      "pot": self.pot,
      "is_running": self.is_running(),
      "players": players,
      "game": self.game.to_broadcast(),
      "me": me,
      "showdown": self.showdown,
    })
  }

  pub fn players_updated(&self) {
    let update_req_message = "[UPDATED]";
    let broadcast_addr = self.udp_port.clone();
    self.socket_udp.send_to(update_req_message.as_bytes(), broadcast_addr).unwrap();
  }

  pub fn player_send(&self, player_id: u64) -> Result<String, String> {
    trace!("Player sending…");
    let node = self.players.get(&player_id).ok_or("Player not found")?;
    let (player_data, stream) = {
      let player_locked = node.try_lock().map_err(|_| "Mutex poisoned")?;
      let data = player_locked.player.to_stream();
      let stream_clone = match &player_locked.stream {
        Some(stream) => Some(stream.try_clone().map_err(|e| e.to_string())?),
        None => None,
      };
      (data, stream_clone)
    };
    
    let response = to_string(&player_data)
      .map_err(|e| format!("Serialization failed: {}", e))?;
    
    let message = format!("[UPDATE] {}\n", response);
    debug!("{}", message);
    if let Some(mut stream) = stream {
      stream.write_all(message.as_bytes()).map_err(|e| format!("Write failed: {}", e))?;
    }
    
    Ok(message)
  }

  // Broadcast Game State via UDP or TCP
  pub fn broadcast_state(&self) {
    let turn_id = self.turn.as_ref().map(|node| node.lock().unwrap().player.id);

    let players: Vec<serde_json::Value> = self.players.values()
      .map(|node| json!(node.lock().unwrap().player.clone().to_broadcast()))
      .collect();

    let game = self.game.to_broadcast();

    let game_state = json!({
      "turn": turn_id,
      "pot": self.pot,
      "players": players,
      "game": game,
    });

    match to_string(&game_state) {
      Ok(json_state) => {
        debug!("Broadcasting game state to all clients on {}: {}", self.udp_port, json_state);
        trace!("{}", self);

        let message = format!("[GAMESTATE] {}\n", json_state);

        if true {
          debug!("Sending multicast to {}", self.udp_port);
          if let Err(e) = self.socket_udp.send_to(message.as_bytes(), &self.udp_port) {
            error!("Failed to send multicast: {}", e);
          }
          return;
        }

        let Some(head) = &self.head else { return; };

        let mut current = Some(head.clone());
        let mut first_iteration = true;

        while let Some(node) = current.take() {
          let player_node = node.lock().unwrap();

          if let Some(stream) = &player_node.stream {
            if let Ok(mut stream) = stream.try_clone() {
              if let Err(e) = stream.write_all(message.as_bytes()) {
                error!("Failed to send game state to player {}: {}", player_node.player.id, e);
              }
            }
          }

          current = player_node.nextf.clone();

          if let Some(next_node) = &current {
            if Arc::ptr_eq(next_node, head) && !first_iteration { break; }
          }
          first_iteration = false;
        }
      },
      Err(e) => {
        error!("Failed to serialize game state: {}", e);
      }
    }
  }
}

impl<G: Game, Db: GameDatabase> fmt::Display for Table<G, Db> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    writeln!(f, "Table State:")?;
    writeln!(f, "Current Bet: {}", self.bet)?;
    writeln!(f, "Pot: {}", self.pot)?;
    writeln!(f, "Game: {}", self.game)?;

    if let Some(head) = &self.head {
      writeln!(f, "Players:")?;
      let mut current = Some(head.clone());
      let mut first_iteration = true;
      while let Some(node) = current.take() {
        let player_node = node.lock().unwrap();
        writeln!(f, "  Player {} ({})", player_node.player.id, player_node.player.name)?;
        writeln!(f, "  Wallet: {}", player_node.player.wallet)?;
        writeln!(f, "  Current Bet: {}", player_node.player.bet)?;
        writeln!(f, "  Folded: {}", player_node.player.folded)?;
        writeln!(f, "  Hand: {}", player_node.player.hand)?;

        // Move to next player in the order
        current = player_node.nextf.clone();
        if let Some(next_node) = &current {
          if Arc::ptr_eq(next_node, head) && !first_iteration { break; }
        }
        first_iteration = false;
      }
    } else {
      writeln!(f, "No players at the table.")?;
    }

    // Display current turn, bettor, and blind
    let turn_info = self.turn.as_ref().map_or("None".to_string(), |n| {
      let player = n.lock().unwrap();
      format!("{} ({})", player.player.id, player.player.name)
    });
    let bettor_info = self.bettor.as_ref().map_or("None".to_string(), |n| {
      let player = n.lock().unwrap();
      format!("{} ({})", player.player.id, player.player.name)
    });
    let blind_info = self.blind.as_ref().map_or("None".to_string(), |n| {
      let player = n.lock().unwrap();
      format!("{} ({})", player.player.id, player.player.name)
    });

    writeln!(f, "Current Turn: {}", turn_info)?;
    writeln!(f, "Current Bettor: {}", bettor_info)?;
    writeln!(f, "Current Blind: {}", blind_info)?;

    Ok(())
  }
}
