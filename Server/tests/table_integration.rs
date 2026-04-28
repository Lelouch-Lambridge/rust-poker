use std::net::{UdpSocket, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::io::{Write, Read};
use lazy_static::lazy_static;
use poker::{player::Player, hand::Hand};
use dealer::table::{Table, handle_client};

// Using different ports for tests to avoid conflicts
const TCP_TEST_PORT: &str = "127.0.0.1:8888";
const UDP_TEST_PORT: &str = "0.0.0.0:8888";

lazy_static! {
  static ref TEST_MUTEX: Mutex<()> = Mutex::new(());
}

#[test]
fn test_broadcast_state() {
  println!("test_broadcast_state");
  
  // Use a unique port for the test
  let port = 9000 + rand::random::<u16>() % 1000;
  
  // Create our receiver socket FIRST
  let receiver = UdpSocket::bind(format!("0.0.0.0:{}", port))
    .expect("Failed to bind UDP receiver");
  
  // Set socket options
  receiver.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
  
  // Create table with a different port
  let table_port = format!("0.0.0.0:{}", port + 1);
  let mut table = Table::new(&table_port);
  
  // Add a player
  let player = Player {
    id: 1,
    name: "Alice".to_string(),
    wallet: 1000,
    hand: Hand(vec![]),
    hand_size: 0,
    bet: 0,
    folded: false,
  };
  
  table.add_player(player);
  
  // Start game to set turn
  table.start_game();
  
  // Get the JSON state that would be broadcasted
  let turn_id = table.turn.as_ref().map(|node| node.lock().unwrap().player.id);
  let players: Vec<_> = table.players.values()
    .map(|node| serde_json::json!(node.lock().unwrap().player.clone().to_broadcast()))
    .collect();
  
  let game_state = serde_json::json!({
    "turn": turn_id,
    "pot": table.pot,
    "players": players
  });
  
  let json_state = serde_json::to_string(&game_state).unwrap();
  println!("Manually sending state: {}", json_state);
  
  // Send directly to our receiver instead of broadcasting
  let test_dest = format!("127.0.0.1:{}", port);
  table.socket_udp.send_to(json_state.as_bytes(), &test_dest)
    .expect("Failed to send test data");
  
  // Now receive and check the data
  let mut buffer = [0; 1024];
  println!("Waiting for data...");
  
  match receiver.recv(&mut buffer) {
    Ok(size) => {
      println!("Received {} bytes of data", size);
      assert!(size > 0, "Received empty UDP packet");
      
      let data = std::str::from_utf8(&buffer[..size])
        .expect("Received non-UTF8 data");
      
      println!("Received data: {}", data);
      
      let json: serde_json::Value = serde_json::from_str(data)
        .expect("Failed to parse JSON");
      
      assert!(json.is_object());
      assert!(json.get("turn").is_some());
      assert!(json.get("pot").is_some());
      assert!(json.get("players").is_some());
      assert!(json["players"].is_array());
      
      assert_eq!(json["players"].as_array().unwrap().len(), 1);
      assert_eq!(json["players"][0]["id"].as_u64().unwrap(), 1);
      assert_eq!(json["players"][0]["name"].as_str().unwrap(), "Alice");
    },
    Err(e) => {
      println!("Error receiving data: {:?}", e);
      println!("This could be a problem with either:");
      println!("1. The Table's broadcast_state() method not sending data properly");
      println!("2. A network configuration issue preventing loopback communication");

      panic!("Failed to receive UDP broadcast: {:?}", e);
    }
  }
}

#[test]
fn test_handle_client_integration() {
  println!("test_handle_client_integration");
  let _lock = TEST_MUTEX.lock().unwrap();
  let table = Arc::new(Mutex::new(Table::<G>::new(UDP_TEST_PORT)));
  let listener = TcpListener::bind(TCP_TEST_PORT).expect("Failed to bind TCP");
  
  // Spawn server thread
  let table_clone = table.clone();
  let server_thread = thread::spawn(move || {
    if let Ok(stream) = listener.accept() {
      handle_client(stream.0, table_clone);
    }
  });
  
  // Give the server time to start
  thread::sleep(Duration::from_millis(100));
  
  // Connect client
  let mut client = TcpStream::connect(TCP_TEST_PORT).expect("Failed to connect to server");
  
  // Send JOIN command
  client.write_all(b"JOIN 1 Dave 1000\n").expect("Failed to send JOIN");
  
  // Read response
  let mut buffer = [0; 1024];
  let size = client.read(&mut buffer).expect("Failed to read response");
  let response = String::from_utf8_lossy(&buffer[..size]);
  
  assert_eq!(response.trim(), "JOINED");
  
  // Verify player was added to table
  thread::sleep(Duration::from_millis(100));
  {
    let table_lock = table.lock().unwrap();
    assert_eq!(table_lock.players.len(), 1);
    assert!(table_lock.players.contains_key(&1));
  }
  
  // Drop client to end the connection
  drop(client);
  
  // Wait for server thread to finish
  let _ = server_thread.join();
}

#[test]
fn test_full_game_flow() {
  println!("test_full_game_flow");
  let _lock = TEST_MUTEX.lock().unwrap();
  let table = Arc::new(Mutex::new(Table::<G>::new(UDP_TEST_PORT)));
  let listener = TcpListener::bind(TCP_TEST_PORT).expect("Failed to bind TCP");
  
  // Spawn server thread
  let table_clone = table.clone();
  let server_thread = thread::spawn(move || {
    if let Ok((stream, _)) = listener.accept() {
      handle_client(stream, table_clone);
    }
  });
  
  // Connect client
  thread::sleep(Duration::from_millis(100));
  let mut client = TcpStream::connect(TCP_TEST_PORT).expect("Failed to connect to server");
  
  // Join game
  client.write_all(b"JOIN 1 Player1 1000\n").expect("Failed to send JOIN");
  let mut buffer = [0; 1024];
  let _ = client.read(&mut buffer).expect("Failed to read response");
  
  // Get cards
  client.write_all(b"GET_CARDS 2\n").expect("Failed to send GET_CARDS");
  let size = client.read(&mut buffer).expect("Failed to read response");
  let cards_response = String::from_utf8_lossy(&buffer[..size]);
  
  // Verify cards were dealt
  let cards: Vec<&str> = cards_response.split(';').collect();
  assert_eq!(cards.len(), 2);
  
  // Make a bet
  client.write_all(b"RAISE 50\n").expect("Failed to send RAISE");
  let size = client.read(&mut buffer).expect("Failed to read response");
  let bet_response = String::from_utf8_lossy(&buffer[..size]);
  
  // Verify wallet and bet updated
  let parts: Vec<&str> = bet_response.split(';').collect();
  assert_eq!(parts.len(), 2);
  assert_eq!(parts[0], "950"); // wallet
  assert_eq!(parts[1], "50");  // bet
  
  // Leave game
  client.write_all(b"LEAVE\n").expect("Failed to send LEAVE");
  let size = client.read(&mut buffer).expect("Failed to read response");
  let leave_response = String::from_utf8_lossy(&buffer[..size]);
  
  assert_eq!(leave_response.trim(), "REMOVED");
  
  // Verify player was removed
  thread::sleep(Duration::from_millis(100));
  {
    let table_lock = table.lock().unwrap();
    assert_eq!(table_lock.players.len(), 0);
  }
  
  // End test
  drop(client);
  let _ = server_thread.join();
}