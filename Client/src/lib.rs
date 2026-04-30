use std::io::{self, Write, Read};
#[allow(unused_imports)]
use std::net::{TcpStream, UdpSocket, SocketAddr, Ipv4Addr};
use std::process;
use socket2::{Socket, Domain, Type, Protocol, SockRef};
use std::time::{SystemTime, UNIX_EPOCH, Duration};
use std::sync::{Arc, Mutex};
use std::hash::{Hasher, DefaultHasher};
use sysinfo::System;
use serde_json::Value;
use poker::{player::Player, hand::Hand, card::Card};
use std::thread::{self, JoinHandle};
use std::collections::HashSet;
use log::{debug, info, warn, error, trace};

#[allow(dead_code)]
pub fn get_hashed_process_id() -> u64 {
  let pid = process::id();
  let start_time = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap()
    .as_millis();

  let mut hasher = DefaultHasher::new();
  hasher.write_u32(pid);
  hasher.write_u128(start_time);
  hasher.finish()
}

#[allow(dead_code)]
pub fn get_device_id() -> u64 {
  let mut sys = System::new();
  sys.refresh_all();

  let sys_name = System::name().unwrap_or_else(|| "unknown".to_string());
  let sys_version = System::os_version().unwrap_or_else(|| "unknown".to_string());
  let host_name = System::host_name().unwrap_or_else(|| "unknown".to_string());

  let mut hasher = DefaultHasher::new();
  hasher.write(sys_name.as_bytes());
  hasher.write(sys_version.as_bytes());
  hasher.write(host_name.as_bytes());

  hasher.finish()
}

#[derive(Debug, Clone)]
pub struct Client {
  pub port: Arc<Mutex<String>>,
  pub my: Arc<Mutex<Player>>,
  pub tcp_stream: Arc<Mutex<TcpStream>>,
  pub udp_socket: Arc<Mutex<UdpSocket>>,
  pub running: Arc<Mutex<bool>>,
  pub available_games: Arc<Mutex<HashSet<String>>>,
  pub multicast_ip: Arc<Mutex<Option<Ipv4Addr>>>,
  pub reconnecting: Arc<Mutex<bool>>,
  tcp_listener: Arc<Mutex<Option<JoinHandle<()>>>>,
  udp_listener: Arc<Mutex<Option<JoinHandle<()>>>>,
}


fn get_local_ip() -> std::net::Ipv4Addr {
  use std::net::UdpSocket;
  let socket = UdpSocket::bind("0.0.0.0:0").expect("Failed to bind dummy socket");
  socket.connect("8.8.8.8:80").expect("Failed to connect dummy socket");
  match socket.local_addr().expect("No local address").ip() {
    std::net::IpAddr::V4(ip) => return ip,
    _ => panic!("Only IPv4 supported"),
  }
}

impl Client {
  pub fn new(port: &str, name: String, wallet: u64, multicast_ip: Ipv4Addr) -> io::Result<Self> {
    debug!("Creating new client connecting to {}", port);
    let server_port = port.split(':').nth(1).unwrap_or("9999");
    let udp_bind_addr = format!("0.0.0.0:{}", server_port);

    let addr: SocketAddr = udp_bind_addr.parse().map_err(|e| {
      error!("Invalid UDP address: {}", e);
      io::Error::new(io::ErrorKind::InvalidInput, format!("Invalid UDP address: {}", e))
    })?;

    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;

    #[cfg(unix)]
    {
      use socket2::SockRef;
      let sockref = SockRef::from(&socket);
      sockref.set_reuse_port(true)?;
    }

    socket.bind(&addr.into())?;
    let udp_socket = UdpSocket::from(socket);

    let local_ip = get_local_ip();
    debug!("Local IP: {}", local_ip);
    udp_socket.join_multicast_v4(&multicast_ip, &local_ip)
      .map_err(|e| {
        error!("Failed to join multicast group: {}", e);
        io::Error::new(io::ErrorKind::Other, format!("Failed to join multicast group: {}", e))
      })?;

    let sockref = SockRef::from(&udp_socket);
    sockref
      .set_multicast_if_v4(&local_ip)
      .map_err(|e| {
        error!("Failed to set multicast interface: {}", e);
        io::Error::new(io::ErrorKind::Other, format!("Failed to set multicast interface: {}", e))
      })?;

    let tcp_stream = Arc::new(Mutex::new(TcpStream::connect(port)?));
    tcp_stream.lock().unwrap().set_nonblocking(true)?;

    let udp_socket = Arc::new(Mutex::new(udp_socket));
    let running = Arc::new(Mutex::new(true));

    info!("Client created successfully with name '{}' and wallet {}", name, wallet);
    Ok(Self {
      port: Arc::new(Mutex::new(port.to_string())),
      my: Arc::new(Mutex::new(Player::new(get_device_id(), name, wallet))),
      tcp_stream, 
      udp_socket, 
      running, 
      available_games: Arc::new(Mutex::new(HashSet::new())),
      multicast_ip: Arc::new(Mutex::new(Some(multicast_ip))),
      reconnecting: Arc::new(Mutex::new(false)),
      tcp_listener: Arc::new(Mutex::new(None)),
      udp_listener: Arc::new(Mutex::new(None)),
    })
  }

  fn games_from_response(&self, response: &str) -> io::Result<()> {
    if !response.contains("[GAMES]") { return Ok(()); }
    
    debug!("Raw games response: {}", response);
    
    let mut all_game_names = HashSet::new();
    let mut remaining = response;
    while let Some(update_pos) = remaining.find("[GAMES]") {
      remaining = &remaining[update_pos + 7..].trim_start();

      let end_pos = remaining.find("[GAMES]").unwrap_or(remaining.len());
      let single_update = &remaining[..end_pos].trim();

      match serde_json::from_str::<Value>(single_update) {
        Ok(json) => {
          trace!("Parsed JSON: {}", json);
          if let Some(games) = json.get("games").and_then(|g| g.as_array()) {
            for val in games {
              if let Some(name) = val.as_str() {
                all_game_names.insert(name.to_string());
              }
            }
          }
        }
        Err(e) => {
          warn!("Failed to parse JSON update: {}", e);
          warn!("Raw update: {}", single_update);
        }
      }

      remaining = &remaining[end_pos..];
    }

    // Store only if changed
    let mut stored = self.available_games.lock().unwrap();
    if *stored != all_game_names {
      *stored = all_game_names;
      info!("Updated available games list");
    }
    Ok(())
  }

  fn update_player_from_response(&self, response: &str) -> io::Result<()> {
    if !response.contains("[UPDATE]") { return Ok(()); }

    debug!("Raw update response: {}", response);
    
    let mut remaining = response;
    while let Some(update_pos) = remaining.find("[UPDATE]") {
      remaining = &remaining[update_pos + 8..];
      
      let end_pos = remaining.find("[UPDATE]").unwrap_or(remaining.len());
      let single_update = &remaining[..end_pos].trim();
      
      match serde_json::from_str::<Value>(single_update) {
        Ok(json) => {
          trace!("Parsed JSON: {}", json);
          
          let mut player = self.my.lock().unwrap();
          
          if let Some(wallet) = json.get("wallet").and_then(|v| v.as_u64()) { player.wallet = wallet; }
          if let Some(bet) = json.get("bet").and_then(|v| v.as_u64()) { player.bet = bet; }
          if let Some(hand) = json.get("hand") {
            if let Some(hand_array) = hand.as_array() {
              let cards: Vec<u8> = hand_array.iter().filter_map(|v| v.as_u64().map(|n| n as u8)).collect();
              player.hand.0 = cards;
            }
          }
        },
        Err(e) => {
          warn!("Failed to parse JSON update: {}", e);
          warn!("Raw update: {}", single_update);
        }
      }
      remaining = &remaining[end_pos..];
    }

    Ok(())
  }

  pub fn send_command(&self, command: &str) -> io::Result<String> {
    let mut stream = self.tcp_stream.lock().unwrap();
    stream.set_nonblocking(false)?;
    
    let response = {
      let cmd = if command.ends_with('\n') {
        command.to_string()
      } else {
        format!("{}\n", command)
      };
      
      debug!("Sending command: {}", command.trim());
      if let Err(e) = stream.write_all(cmd.as_bytes()) {
        error!("Failed to send command: {}, attempting reconnect...", e);
        drop(stream);
        self.reconnect()?;
        return Err(io::Error::new(io::ErrorKind::BrokenPipe, "Lost connection to server"));
      }
      
      if let Err(e) = stream.flush() {
        error!("Failed to flush stream: {}", e);
        drop(stream);
        self.reconnect()?;
        return Err(io::Error::new(io::ErrorKind::BrokenPipe, "Failed to flush data to server"));
      }
      
      let mut buffer = [0; 1024];
      match stream.read(&mut buffer) {
        Ok(size) => {
          let resp = String::from_utf8_lossy(&buffer[..size]).to_string();
          trace!("Received response: {}", resp);
          resp
        }
        Err(e) => {
          error!("Failed to read response: {}, attempting reconnect...", e);
          drop(stream);
          self.reconnect()?;
          return Err(io::Error::new(io::ErrorKind::BrokenPipe, "Lost connection to server"));
        }
      }
    };
    
    stream.set_nonblocking(true)?;
    drop(stream);
  
    if response.contains("NOT YOUR TURN") {
      info!("It's not your turn yet. Please wait.");
      return Err(io::Error::new(io::ErrorKind::Other, "Not your turn"));
    }
    
    if let Err(e) = self.update_player_from_response(&response) {
      warn!("Failed to update player state: {}", e);
    }

    Ok(response)
  }

  pub fn reconnect(&self) -> io::Result<()> {
    info!("Reconnecting to server...");
    let new_stream = TcpStream::connect(&self.port.lock().unwrap().clone())?;
    new_stream.set_nonblocking(true)?;
    *self.tcp_stream.lock().unwrap() = new_stream;
    info!("Reconnected successfully.");
    Ok(())
  }

  pub fn is_connected(&self) -> bool { 
    if let Ok(stream) = self.tcp_stream.lock() {
      let _ = stream.set_nonblocking(false);
    }

    let result = self.send_command("PING").is_ok();

    if let Ok(stream) = self.tcp_stream.lock() {
      let _ = stream.set_nonblocking(true);
    }

    result
  }

  pub fn join(&self) {
    let player_id;
    let player_name;
    let player_wallet;
    
    {
      let player = self.my.lock().unwrap();
      player_id = player.id;
      player_name = player.name.clone();
      player_wallet = player.wallet;
    }
    
    info!("Joining game as {} (ID: {}) with wallet {}", player_name, player_id, player_wallet);
    let result = self.send_command(&format!("JOIN {} {} {}", player_id, player_name, player_wallet));
    match result {
      Ok(response) => info!("Server Response: {}", response),
      Err(e) => error!("Failed to join game: {}", e)
    };
  }

  pub fn leave_game(&self) {
    info!("Leaving game");
    let result = self.send_command("LEAVE");
    match result {
      Ok(response) => info!("Server Response: {}", response),
      Err(e) => error!("Failed to leave game: {}", e)
    };
  }

  pub fn replace_cards(&self, discard: Vec<u8>) {
    let cards_str = discard.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(" ");
    let command = format!("REPLACE_CARDS {}", cards_str);

    info!("Replacing cards: {}", cards_str);
    let result = self.send_command(&command);
    match result {
      Ok(response) => {
        if response.starts_with("[ERROR]") {
          warn!("{}", response);
          return;
        }
        
        if response.starts_with("[UPDATE]") {
          let hand_str = {
            let player = self.my.lock().unwrap();
            player.hand.to_string()
          };
          info!("Cards replaced successfully. New hand: {}", hand_str);
        } else {
          info!("Server Response: {}", response);
        }
      },
      Err(e) => error!("Failed to replace cards: {}", e)
    };
  }

  pub fn raise(&self, amount: u64) {
    let can_raise = {
      let player = self.my.lock().unwrap();
      if player.wallet < amount {
        warn!("Not Enough Money, Max amount: {}", player.wallet);
        false
      } else { true }
    };
    
    if !can_raise { return; }
    
    info!("Raising {}", amount);
    let result = self.send_command(&format!("RAISE {}", amount));
    match result {
      Ok(response) if response.starts_with("[ERROR]") => {
        warn!("{}", response);
        return;
      },
      Ok(msg) => info!("OK: {}", msg),
      Err(e) => error!("Failed to raise: {}", e),
    };
  }

  pub fn check(&self) {
    info!("Checking");
    let result = self.send_command("CHECK");
    match result {
      Ok(response) if response.starts_with("[ERROR]") => {
        warn!("{}", response);
        return;
      },
      Ok(msg) => info!("OK: {}", msg),
      Err(e) => error!("Failed to check: {}", e),
    };
  }

  pub fn all_in(&self) {
    info!("Going all in");
    let result = self.send_command("ALL_IN");
    match result {
      Ok(response) if response.starts_with("[ERROR]") => {
        warn!("{}", response);
        return;
      },
      Ok(msg) => info!("OK: {}", msg),
      Err(e) => error!("Failed to go all in: {}", e),
    };
  }

  pub fn fold(&self) {
    info!("Folding");
    let result = self.send_command("FOLD");
    match result {
      Ok(response) => {
        if response.starts_with("[ERROR]") {
          warn!("{}", response);
          return;
        }

        info!("Fold successful.");
      },
      Err(e) => error!("Failed to fold: {}", e)
    };
  }

  pub fn process_game_state(&self, json_str: &str) -> io::Result<()> {
    if !json_str.contains("[GAMESTATE]") && !json_str.contains("{\"turn\":") {
      return Ok(());
    }
    
    let json_content = if json_str.contains("[GAMESTATE]") {
      match json_str.split("[GAMESTATE]").nth(1) {
        Some(content) => content.trim(),
        None => json_str
      }
    } else {
      json_str
    };
    
    match serde_json::from_str::<Value>(json_content) {
      Ok(parsed) => {
        debug!("Received game state update");
        
        let mut game_summary = String::new();
        
        if let Some(turn) = parsed.get("turn") {
          game_summary.push_str(&format!("Current turn: Player {}\n", turn));
        }
        
        if let Some(pot) = parsed.get("pot") {
          game_summary.push_str(&format!("Pot: {}", pot));
        }
        
        println!("\n=== Game State Update ===\n{}", game_summary);

        if let Some(players) = parsed.get("players").and_then(|p| p.as_array()) {
          println!("Players:");
          for player in players {
            let (Some(id), Some(name)) = (player.get("id"), player.get("name")) else { continue; };
            let hand = player.get("hand").and_then(|h| h.as_array())
              .map(|arr| {
                arr.iter()
                  .filter_map(|v| v.as_u64().map(|n| n as u8))
                  .collect::<Vec<u8>>()
              }).unwrap_or_else(Vec::new);
            let wallet = player.get("wallet").and_then(|w| w.as_u64()).unwrap_or(0);
            let bet = player.get("bet").and_then(|b| b.as_u64()).unwrap_or(0);
            let folded = player.get("folded").and_then(|f| f.as_bool()).unwrap_or(false);

            println!("  {} ({}):\n  Wallet={},\n  Bet={},\n  Folded={},\n  Hand={}", 
                name, id, wallet, bet, folded, Hand(hand));
          }
        }

        if let Some(game) = parsed.get("game").and_then(|g| g.as_object()) {
          println!("Game State:");
          if let Some(round) = game.get("round") {
            println!("  Round: {}", round);
          }
          if let Some(community_cards) = game.get("community_cards").and_then(|cc| cc.as_array()) {
            let cards: Vec<u8> = community_cards.iter()
              .filter_map(|v| v.as_u64().map(|n| n as u8))
              .collect();
            println!("  Community Cards: {}", Hand(cards));
          }
        }
        println!("=========================\n");

        Ok(())
      },
      Err(e) => {
        warn!("Received invalid game state: {}", e);
        debug!("Raw data: {}", json_content);
        Err(io::Error::new(io::ErrorKind::InvalidData, "Failed to parse game state"))
      }
    }
  }

  pub fn take_lobby_input(&self) -> Option<(String, String)> {
    use std::time::{Duration, Instant};

    let start = Instant::now();
    let games: Vec<String> = loop {
      let locked = self.available_games.lock().unwrap();
      if !locked.is_empty() {
        let mut sorted: Vec<String> = locked.iter().cloned().collect();
        sorted.sort();
        break sorted;
      }

      if start.elapsed().as_secs() > 10 {
        error!("No games available after waiting");
        return None;
      }

      std::thread::sleep(Duration::from_millis(100));
    };

    println!("  0: Exit program");
    println!("Available games:");
    for (i, game) in games.iter().enumerate() {
      println!("  {}: {}", i + 1, game);
    }

    println!("Enter the number of the game to join (or 0 to exit):");
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
      error!("Failed to read input");
      return None;
    }

    let choice = input.trim().parse::<usize>().unwrap_or(0);
    
    if choice == 0 {
      return Some(("EXIT".to_string(), "EXIT".to_string()));
    }
    
    if choice > games.len() {
      error!("Invalid selection");
      return None;
    }

    let selected_game = &games[choice - 1];
    info!("Selected game: {}", selected_game);

    match self.send_command(selected_game) {
      Ok(response) => {
        if response.contains("[GAME_INFO]") {
          if let Some(json_str) = response.split("[GAME_INFO]").nth(1) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(json_str.trim()) {
              let port = json.get("port")?.as_u64()? as u16;
              let multicast = json.get("multicast")?.as_str()?.to_string();
              let addr = self.tcp_stream.lock().ok()?.peer_addr().ok()?.ip().to_string();
              let tcp_target = format!("{}:{}", addr, port);
              info!("Game info received: TCP={}, multicast={}", tcp_target, multicast);
              return Some((tcp_target, multicast));
            }
          }
          error!("Malformed GAME_INFO response");
          None
        } else {
          error!("Did not receive GAME_INFO");
          None
        }
      }
      Err(e) => {
        error!("Failed to request game info: {}", e);
        None
      }
    }
  }

  pub fn take_input(&self) {
    loop {
      println!("\nEnter command:");
      println!("  REPLACE/RE: Replace cards");
      println!("  RAISE/R <amount>: Raise bet");
      println!("  CHECK/C: Check");
      println!("  ALL_IN/ALLIN/A: Bet your remaining wallet");
      println!("  FOLD/F: Fold hand");
      println!("  UPDATE/U: Update info");
      println!("  PRINT/P: Show current status");
      println!("  LEAVE/L: Leave game");
      
      let mut input = String::new();
      io::stdin().read_line(&mut input).unwrap();
      let input_parts: Vec<&str> = input.trim().split_whitespace().collect();
      
      if input_parts.is_empty() {
        info!("No command entered");
        continue;
      }
      
      let command = input_parts[0].to_uppercase();
      
      match command.as_str() {
        "REPLACE" | "RE" => {
          let hand_cards;
          
          let player = self.my.lock().unwrap();
          hand_cards = player.hand.0.clone();
          drop(player);

          println!("Your current hand:");
          for (i, card) in hand_cards.iter().enumerate() {
            println!("  {}: {}", i, Card(*card));
          }

          println!("Enter indices of cards to discard (e.g., '0 2 4'):");
          let mut input = String::new();
          io::stdin().read_line(&mut input).unwrap();
          let indices: Vec<usize> = input
            .trim()
            .split_whitespace()
            .filter_map(|s| s.parse::<usize>().ok())
            .collect();

          let mut discard = vec![];
          for idx in indices {
            if let Some(&card) = hand_cards.get(idx) {
              discard.push(card);
            } else {
              warn!("Invalid card index: {}", idx);
            }
          }

          if discard.is_empty() {
            info!("You chose to keep all your cards");
          }

          self.replace_cards(discard);
        }
        "RAISE" | "R" => {
          let amount = if input_parts.len() > 1 {
            match input_parts[1].parse::<u64>() {
              Ok(val) => val,
              Err(_) => {
                warn!("Invalid amount. Using default of 50");
                50
              }
            }
          } else {
            info!("No amount specified. Using default of 50");
            50
          };
          
          self.raise(amount);
        }
        "CHECK" | "C" => {
          self.check();
        }
        "ALLIN" | "ALL_IN" | "A" => {
          self.all_in();
        }
        "FOLD" | "F" => {
          self.fold();
        }
        "UPDATE" | "U" => {
          info!("Requesting update");
          match self.send_command("UPDATE") {
            Ok(response) => {
              debug!("Update response: {}", response);
              
              let player = self.my.lock().unwrap();
              println!("  ID: {}", player.id);
              println!("  Name: {}", player.name);
              println!("  Wallet: {}", player.wallet);
              println!("  Current bet: {}", player.bet);
              println!("  Hand: {}", player.hand);
            },
            Err(e) => error!("Failed to update: {}", e)
          };
        },
        "PRINT" | "P" => {
          info!("Player status:");
          
          let player = self.my.lock().unwrap();
          println!("  ID: {}", player.id);
          println!("  Name: {}", player.name);
          println!("  Wallet: {}", player.wallet);
          println!("  Current bet: {}", player.bet);
          println!("  Hand: {}", player.hand);
        },
        "LEAVE" | "L" => {
          self.leave_game();
          break;
        }
        _ => warn!("Unknown command: {}", command),
      }
    }
  }

  // Add this to the Client implementation in lib.rs
  pub fn reconnect_to(&self, tcp_target: String, multicast_ip: String) -> io::Result<()> {
    {
      let mut reconnecting = self.reconnecting.lock().unwrap();
      *reconnecting = true;
    }
    
    info!("Starting reconnection to {} with multicast {}", tcp_target, multicast_ip);
    
    // First properly stop all listeners
    self.stop_listeners()?;
    
    // Ensure we leave any existing multicast group if possible
    if let Some(current_ip) = *self.multicast_ip.lock().unwrap() {
      info!("Attempting to leave previous multicast group: {}", current_ip);
      let local_ip = get_local_ip();
      
      // Try to leave the previous multicast group, but don't fail if it errors
      if let Ok(socket_guard) = self.udp_socket.lock() {
        let _ = socket_guard.leave_multicast_v4(&current_ip, &local_ip);
        info!("Left previous multicast group");
      }
    }
    
    // Perform the actual reconnection
    let result = self.do_reconnect(tcp_target, multicast_ip);
    
    if result.is_ok() {
      // Only restart the listeners if reconnection was successful
      self.start_listeners();
      info!("Listeners restarted after successful reconnection");
    }
    
    {
      let mut reconnecting = self.reconnecting.lock().unwrap();
      *reconnecting = false;
    }
    
    result
  }

  // Modify the do_reconnect function to ensure better cleanup
  fn do_reconnect(&self, tcp_target: String, multicast_ip: String) -> io::Result<()> {
    debug!("Starting reconnect to {} with multicast {}", tcp_target, multicast_ip);
    
    debug!("Step 1: Parsing new multicast IP");
    let new_ip: Ipv4Addr = match multicast_ip.parse() {
      Ok(ip) => {
        debug!("Successfully parsed multicast IP: {}", ip);
        ip
      },
      Err(e) => {
        let err = io::Error::new(io::ErrorKind::InvalidInput, 
          format!("Invalid multicast IP: {}", e));
        error!("Failed to parse multicast IP: {}", e);
        return Err(err);
      }
    };
    
    debug!("Step 2: Connecting to TCP server at {}", tcp_target);
    let new_stream = match TcpStream::connect(&tcp_target) {
      Ok(s) => s,
      Err(e) => {
        error!("Failed to connect to TCP server: {}", e);
        return Err(e);
      }
    };
    
    if let Err(e) = new_stream.set_nonblocking(true) {
      warn!("Failed to set non-blocking mode: {}", e);
    }
    
    debug!("Step 3: Creating new UDP socket");
    let socket = match Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)) {
      Ok(s) => s,
      Err(e) => {
        error!("Failed to create UDP socket: {}", e);
        return Err(e);
      }
    };

    if let Err(e) = socket.set_reuse_address(true) {
      error!("Failed to set reuse_address: {}", e);
      return Err(e);
    }

    #[cfg(unix)]
    {
      if let Err(e) = SockRef::from(&socket).set_reuse_port(true) {
        error!("Failed to set reuse_port: {}", e);
        return Err(e);
      }
    }

    debug!("Step 4: Binding UDP socket");
    let server_port = tcp_target.split(':').nth(1).unwrap_or("9999");
    let udp_bind_addr = format!("0.0.0.0:{}", server_port);
    let addr: SocketAddr = match udp_bind_addr.parse() {
      Ok(a) => a,
      Err(e) => {
        let err = io::Error::new(io::ErrorKind::InvalidInput, 
          format!("Invalid UDP address: {}", e));
        error!("Failed to parse UDP bind address: {}", e);
        return Err(err);
      }
    };

    if let Err(e) = socket.bind(&addr.into()) {
      error!("Failed to bind UDP socket: {}", e);
      return Err(e);
    }

    let new_udp_socket = UdpSocket::from(socket);
    
    debug!("Step 5: Joining new multicast group");
    let local_ip = get_local_ip();
    debug!("Local IP for multicast: {}", local_ip);
    
    if let Err(e) = new_udp_socket.join_multicast_v4(&new_ip, &local_ip) {
      error!("Failed to join multicast group: {}", e);
      return Err(io::Error::new(io::ErrorKind::Other, 
        format!("Failed to join multicast: {}", e)));
    }
    
    let sockref = SockRef::from(&new_udp_socket);
    if let Err(e) = sockref.set_multicast_if_v4(&local_ip) {
      error!("Failed to set multicast interface: {}", e);
      return Err(e);
    }
    
    debug!("Successfully joined multicast group: {}", new_ip);
    
    // Update all client state in a synchronized way
    {
      debug!("Updating TCP stream");
      *self.tcp_stream.lock().unwrap() = new_stream;
      
      debug!("Updating port");
      *self.port.lock().unwrap() = tcp_target.clone();
      
      debug!("Updating multicast IP");
      *self.multicast_ip.lock().unwrap() = Some(new_ip);
      
      debug!("Updating UDP socket");
      *self.udp_socket.lock().unwrap() = new_udp_socket;
    }
    
    info!("Successfully reconnected to {}", tcp_target);
    Ok(())
  }

  pub fn start_listeners(&self) {
    self.start_tcp_listener();
    self.start_udp_listener();
  }

  pub fn start_tcp_listener(&self) {
    let mut tcp_handle = self.tcp_listener.lock().unwrap();

    if tcp_handle.is_some() {
      info!("TCP listener already running");
      return;
    }

    info!("Starting TCP listener...");

    let client_clone = self.clone();

    *tcp_handle = Some(thread::spawn(move || {
      client_clone.listen_for_tcp();
    }));
  }

  pub fn start_udp_listener(&self) {
    let mut udp_handle = self.udp_listener.lock().unwrap();

    if udp_handle.is_some() {
      info!("UDP listener already running");
      return;
    }

    info!("Starting UDP listener...");

    let client_clone = self.clone();

    *udp_handle = Some(thread::spawn(move || {
      client_clone.listen_for_udp();
    }));
  }

  pub fn stop_listeners(&self) -> io::Result<()> {
    info!("Stopping listeners for reconnection...");

    {
      let mut running = self.running.lock().unwrap();
      *running = false;
    }

    {
      let mut tcp_handle = self.tcp_listener.lock().unwrap();
      if let Some(handle) = tcp_handle.take() {
        info!("Waiting for TCP listener thread to finish...");
        let _ = handle.join(); // we ignore the result since panics are already logged
      }
      *tcp_handle = None;
    }

    {
      let mut udp_handle = self.udp_listener.lock().unwrap();
      if let Some(handle) = udp_handle.take() {
        info!("Waiting for UDP listener thread to finish...");
        let _ = handle.join();
      }
      *udp_handle = None;
    }

    {
      let mut running = self.running.lock().unwrap();
      *running = true;
    }

    info!("All listeners stopped.");
    Ok(())
  }

  pub fn listen_for_tcp(&self) {
    info!("TCP listener thread started.");
    
    let running = self.running.clone();
    let tcp_stream = self.tcp_stream.clone();
    let reconnecting = self.reconnecting.clone();
    let mut buffer = [0; 1024];
    
    while *running.lock().unwrap() {
      if *reconnecting.lock().unwrap() {
        thread::sleep(Duration::from_millis(100));
        continue;
      }
      
      let mut stream = match tcp_stream.lock() {
        Ok(stream) => stream,
        Err(e) => {
          error!("Failed to lock TCP stream: {}", e);
          thread::sleep(Duration::from_millis(100));
          continue;
        }
      };
      
      match stream.read(&mut buffer) {
        Ok(size) if size > 0 => {
          let response = String::from_utf8_lossy(&buffer[..size]).to_string();

          if response.contains("[UPDATE]") {
            if let Err(e) = self.update_player_from_response(&response) {
              warn!("Failed to process update in TCP listener: {}", e);
            } else {
              println!("\n[SERVER UPDATE] Received player update");
              
              let player = self.my.lock().unwrap();
              println!("Updated wallet: {}", player.wallet);
              println!("Updated bet: {}", player.bet);
              println!("Updated hand: {}", player.hand);
            }
          } else if response.contains("[GAMESTATE]") {
            trace!("TCP:");
            if let Err(e) = self.process_game_state(&response) {
              warn!("Failed to process game state in TCP listener: {}", e);
            }
          } else if response.contains("[GAMES]") {
            if let Err(e) = self.games_from_response(&response) {
              warn!("Failed to process games in TCP listener: {}", e);
              warn!("Raw games response: {}", response);
            }
          } else if !response.trim().is_empty() {
            println!("\n[SERVER MESSAGE] {}", response);
          }
        },
        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
          drop(stream);
          if !*running.lock().unwrap() {
            break;
          }
          thread::sleep(Duration::from_millis(100));
        },
        Err(e) => {
          error!("TCP listener error: {}", e);
          drop(stream);
          if !*running.lock().unwrap() {
            break;
          }
          thread::sleep(Duration::from_millis(1000));
        },
        _ => {}
      }
      
      if !*running.lock().unwrap() {
        break;
      }
    }
    info!("TCP listener thread stopped.");
  }

  pub fn listen_for_udp(&self) {
    info!("UDP listener thread started.");
    
    let running = self.running.clone();
    let reconnecting = self.reconnecting.clone();
    let mut buffer = [0; 2048];
    
    while *running.lock().unwrap() {
      if *reconnecting.lock().unwrap() {
        thread::sleep(Duration::from_millis(100));
        continue;
      }
      
      match self.udp_socket.lock().unwrap().set_read_timeout(Some(Duration::from_millis(100))) {
        Ok(_) => {},
        Err(e) => {
          warn!("Failed to set UDP read timeout: {}", e);
        }
      }
      
      match self.udp_socket.lock().unwrap().recv_from(&mut buffer) {
        Ok((size, _)) => {
          let message = String::from_utf8_lossy(&buffer[..size]);

          if message.contains("[UPDATED]") {
            info!("Received update notification via UDP, sending UPDATE request via TCP...");
            match self.send_command("UPDATE") {
              Ok(response) => {
                info!("Update response received: {}", response);
                
                let player = self.my.lock().unwrap();
                println!("  ID: {}", player.id);
                println!("  Name: {}", player.name);
                println!("  Wallet: {}", player.wallet);
                println!("  Current bet: {}", player.bet);
                println!("  Hand: {}", player.hand);
              },
              Err(e) => error!("Failed to send update request: {}", e)
            };
          } else {
            trace!("UDP:");
            if let Err(e) = self.process_game_state(&message) {
              warn!("Failed to process game state from UDP: {}", e);
              println!("Raw data: {}", message);
            }
          }
        },
        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => {
          if !*running.lock().unwrap() {
            break;
          }
          thread::sleep(Duration::from_millis(100));
        },
        Err(e) => {
          warn!("UDP receive error: {}", e);
          if !*running.lock().unwrap() {
            break;
          }
          thread::sleep(Duration::from_millis(500));
        }
      }
      
      if !*running.lock().unwrap() {
        break;
      }
    }
    
    info!("UDP listener thread stopped.");
  }
}

pub fn discover_lobby(multicast_addr: &str, port: u16) -> Option<String> {
  let socket = UdpSocket::bind(format!("0.0.0.0:{}", port)).expect("bind failed");
  socket.join_multicast_v4(
    &multicast_addr.parse().unwrap(),
    &"0.0.0.0".parse().unwrap(),
  ).expect("join_multicast_v4 failed");

  let mut buf = [0u8; 128];
  info!("Waiting for lobby broadcast...");

  match socket.recv_from(&mut buf) {
    Ok((len, _)) => {
      let msg = String::from_utf8_lossy(&buf[..len]);
      if msg.starts_with("LOBBY:") {
        let parts: Vec<&str> = msg.trim().split(':').collect();
        if parts.len() == 3 {
          let ip = parts[1];
          let port = parts[2];
          let full_addr = format!("{}:{}", ip, port);
          info!("Discovered lobby at {}", full_addr);
          return Some(full_addr);
        }
      }
    }
    Err(e) => {
      warn!("Failed to receive lobby discovery: {}", e);
    }
  }
  None
}
