use std::net::{TcpListener, TcpStream, UdpSocket};
use std::io::{Read, Write};
use std::thread;
use std::time::Duration;
use serde_json::json;
use client::{Client, get_device_id, get_hashed_process_id};

struct MockServer {
  tcp_listener: TcpListener,
  udp_socket: UdpSocket,
}

impl MockServer {
  fn new(port: &str) -> Self {
    let tcp_listener = TcpListener::bind(port)
      .expect("Failed to bind TCP listener");
    
    let udp_socket = UdpSocket::bind(port)
      .expect("Failed to bind UDP socket");
    
    MockServer { tcp_listener, udp_socket }
  }

  fn run(&self, response_map: std::collections::HashMap<String, String>) -> Vec<thread::JoinHandle<()>> {
    let tcp_listener = self.tcp_listener.try_clone().unwrap();
    let udp_socket = self.udp_socket.try_clone().unwrap();
    
    // Return thread handles
    vec![
      thread::spawn(move || {
        for stream in tcp_listener.incoming() {
          let response_map = response_map.clone();
          thread::spawn(move || { // Handle each connection in a new thread
            match stream {
              Ok(mut stream) => {
                let mut buffer = [0; 1024];
                if let Ok(size) = stream.read(&mut buffer) {
                  let request = String::from_utf8_lossy(&buffer[..size]).to_string();
                  println!("Req: {}", request);
                  // Generate response
                  let response = response_map.get(&request).cloned().unwrap_or_else(|| {
                    if request.starts_with("JOIN") {
                      "JOIN_SUCCESS".into()
                    } else if request.starts_with("GET_CARDS") {
                      "1;2;3;4".into()
                    } else if request.starts_with("RAISE") {
                      "[UPDATE]".into()
                    } else if request.starts_with("CHECK") {
                      "[UPDATE]".into()
                    } else if request.starts_with("FOLD") {
                      "FOLD_SUCCESS".into()
                    } else if request.starts_with("PING") {
                      "PONG".into()
                    } else {
                      "BALLS".into()
                    }
                  });
                
                  println!("Res: {}", response);
                  // Write and flush immediately
                  stream.write_all(response.as_bytes()).expect("Write failed");
                  stream.flush().expect("Flush failed");
                }
              }
              Err(e) => eprintln!("Connection failed: {}", e),
            }
          });
        }
      }),
      thread::spawn(move || {
        let game_state = json!({
          "pot": 100,
          "current_player": "Player1",
          "players": [
            { "name": "Player1", "wallet": 950, "bet": 50 },
            { "name": "Player2", "wallet": 900, "bet": 100 }
          ]
        }).to_string();
        
        thread::sleep(Duration::from_millis(1));
        udp_socket.send_to(game_state.as_bytes(), "127.0.0.1:9999").unwrap();
      })
    ]
  }
}

#[test]
fn test_get_hashed_process_id() {
  let id1 = get_hashed_process_id();
  thread::sleep(Duration::from_millis(5));
  let id2 = get_hashed_process_id();
  
  assert_ne!(id1, id2);
}

#[test]
fn test_get_device_id() {
  let id1 = get_device_id();
  let id2 = get_device_id();
  
  assert_eq!(id1, id2);
}

#[test]
fn test_client_new() {
  let server_port = "127.0.0.1:9990";
  let mock_server = MockServer::new(server_port);
  mock_server.run(std::collections::HashMap::new());
  
  thread::sleep(Duration::from_millis(50));
  
  let client = Client::new(server_port, "TestPlayer".to_string(), 1000);
  assert!(client.is_ok());
  
  let client = client.unwrap();
  assert_eq!(client.port, server_port);
  assert_eq!(client.my.lock().unwrap().name, "TestPlayer");
  assert_eq!(client.my.lock().unwrap().wallet, 1000);
}

#[test]
fn test_client_send_command() {
  let server_port = "127.0.0.1:9991";
  let mock_server = MockServer::new(server_port);
  
  mock_server.run(std::collections::HashMap::new());
  
  thread::sleep(Duration::from_millis(50));
  
  let client = Client::new(server_port, "TestPlayer".to_string(), 1000).unwrap();
  let response = client.send_command("PING");
  
  assert!(response.is_ok());
  assert_eq!(response.unwrap(), "PONG");
}

#[test]
fn test_client_is_connected() {
  let server_port = "127.0.0.1:9992";
  let mock_server = MockServer::new(server_port);
  
  let mut response_map = std::collections::HashMap::new();
  response_map.insert("PING".to_string(), "PONG".to_string());
  mock_server.run(response_map);
  
  thread::sleep(Duration::from_millis(50));
  
  let client = Client::new(server_port, "TestPlayer".to_string(), 1000).unwrap();
  assert!(client.is_connected());
}

#[test]
fn test_client_join_game() {
  let server_port = "127.0.0.1:9993";
  let mock_server = MockServer::new(server_port);
  
  let response_map = std::collections::HashMap::new();
  mock_server.run(response_map);
  
  thread::sleep(Duration::from_millis(50));
  
  let client = Client::new(server_port, "TestPlayer".to_string(), 1000).unwrap();
  client.join_game();
}

#[test]
fn test_client_raise() {
  let server_port = "127.0.0.1:9995";
  let mock_server = MockServer::new(server_port);

  let mut response_map = std::collections::HashMap::new();
  response_map.insert(
    "RAISE 100\n".to_string(),
    format!(
      "[UPDATE]{}\n",
      json!({ "wallet": 900, "bet": 100 }).to_string()
    ),
  );
  mock_server.run(response_map);

  thread::sleep(Duration::from_millis(50));

  let client = Client::new(server_port, "TestPlayer".to_string(), 1000).unwrap();
  client.raise(100);

  let player = client.my.lock().unwrap();
  assert_eq!(player.wallet, 900);
  assert_eq!(player.bet, 100);
}

#[test]
fn test_client_check() {
  let server_port = "127.0.0.1:9996";
  let mock_server = MockServer::new(server_port);

  let mut response_map = std::collections::HashMap::new();
  response_map.insert(
    "CHECK\n".to_string(),
    format!(
      "[UPDATE]{}\n",
      json!({ "wallet": 980, "bet": 20 }).to_string()
    ),
  );
  mock_server.run(response_map);

  thread::sleep(Duration::from_millis(50));

  let client = Client::new(server_port, "TestPlayer".to_string(), 1000).unwrap();
  client.check();

  let player = client.my.lock().unwrap();
  assert_eq!(player.wallet, 980);
  assert_eq!(player.bet, 20);
}

#[test]
fn test_client_fold() {
  let server_port = "127.0.0.1:9997";
  let mock_server = MockServer::new(server_port);

  let mut response_map = std::collections::HashMap::new();
  response_map.insert("FOLD\n".to_string(), "[UPDATE]{}".to_string());
  mock_server.run(response_map);

  thread::sleep(Duration::from_millis(50));

  let client = Client::new(server_port, "TestPlayer".to_string(), 1000).unwrap();
  client.fold();
}


#[test]
fn test_client_leave_game() {
  let server_port = "127.0.0.1:9998";
  let mock_server = MockServer::new(server_port);
  
  let mut response_map = std::collections::HashMap::new();
  response_map.insert("LEAVE".to_string(), "[UPDATE]{}".to_string().replace("{}", &json!({ "wallet": 1000 }).to_string()));
  mock_server.run(response_map);
  
  thread::sleep(Duration::from_millis(50));
  
  let client = Client::new(server_port, "TestPlayer".to_string(), 1000).unwrap();
  client.leave_game();
}

#[test]
fn test_client_reconnect() {
  let server_port = "127.0.0.1:10000";
  let mock_server = MockServer::new(server_port);
  mock_server.run(std::collections::HashMap::new());
  
  thread::sleep(Duration::from_millis(50));
  
  let client = Client::new(server_port, "TestPlayer".to_string(), 1000).unwrap();
  
  let broken_stream = TcpStream::connect(server_port).unwrap();
  drop(broken_stream); // Close it
  
  assert!(client.reconnect().is_ok());
  assert!(client.is_connected());
}

#[test]
fn test_udp_listener() {
  let server_port = "127.0.0.1:10002";
  let mock_server = MockServer::new(server_port);
  mock_server.run(std::collections::HashMap::new());
  
  thread::sleep(Duration::from_millis(50));
  
  let client = Client::new(server_port, "TestPlayer".to_string(), 1000).unwrap();
  
  let _original_stdout = std::io::stdout();
  let _client_clone = client.clone();

  let handle = thread::spawn(move || {
    let udp_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let game_state = json!({
      "pot": 200,
      "current_player": "Player1"
    }).to_string();
    
    udp_socket.send_to(game_state.as_bytes(), server_port).unwrap();
    thread::sleep(Duration::from_millis(100));
  });
  
  thread::sleep(Duration::from_millis(200));
  
  handle.join().unwrap();
}

// TODO: Fix the test
#[test]
fn test_client_game_flow() {
  let server_port = "127.0.0.1:10001";
  let mock_server = MockServer::new(server_port);

  let player_id = std::process::id() as u64;
  let player_name = "TestPlayer";
  let wallet = 1000;
  let join_command = format!("JOIN {} {} {}\n", player_id, player_name, wallet);

  let mut response_map = std::collections::HashMap::new();
  response_map.insert(
    join_command.clone(),
    format!(
      "[UPDATE]{}\n",
      serde_json::json!({
        "wallet": wallet,
        "bet": 0,
        "hand": [1, 2, 3, 4]
      })
      .to_string()
    ),
  );

  // Start mock server
  let server_threads = mock_server.run(response_map);
  thread::sleep(Duration::from_millis(100));

  let client = Client::new(server_port, player_name.to_string(), wallet).unwrap();
  client.join_game();

  thread::sleep(Duration::from_millis(100));

  let hand = &client.my.lock().unwrap().hand.0;
  assert_eq!(hand, &vec![1, 2, 3, 4]);

  for handle in server_threads {
    handle.join().unwrap();
  }
}