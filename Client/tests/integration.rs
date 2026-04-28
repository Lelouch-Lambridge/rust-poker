use std::sync::mpsc;
use std::io::{BufRead, BufReader};
use std::net::{TcpListener, UdpSocket};
use std::thread;
use std::time::Duration;
use serde_json::json;
use client::Client;
use std::io::Write;

struct TestServer {
  tcp_socket: TcpListener,
  udp_socket: UdpSocket,
}

impl TestServer {
  fn new(addr: &str) -> Self {
    let tcp_socket = TcpListener::bind(addr).unwrap();
    let udp_socket = UdpSocket::bind(addr).unwrap();
    
    TestServer {
      tcp_socket,
      udp_socket,
    }
  }
  
  fn run(&self) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    let tcp_socket = self.tcp_socket.try_clone().unwrap();
    
    thread::spawn(move || {
      if let Ok((mut stream, _)) = tcp_socket.accept() {
        let mut reader = BufReader::new(&stream);
        let mut line = String::new();
        
        if let Ok(_) = reader.read_line(&mut line) {
          tx.send(line.clone()).unwrap();

          if line.starts_with("JOIN") {
            let _ = stream.write_all(b"[UPDATE]{\"wallet\":1000,\"bet\":0}");
            let _ = stream.flush();
          } else if line.starts_with("GET_CARDS") {
            let _ = stream.write(b"1;2;3;4");
            let _ = stream.flush();
          } else {
            let _ = stream.write(b"OK");
            let _ = stream.flush();
          }
        }
      }
    });
    
    rx
  }
  
  fn send_game_update(&self, client_addr: &str) {
    let game_state = json!({
      "pot": 150,
      "players": [
        {"name": "Player1", "wallet": 900, "bet": 100},
        {"name": "Player2", "wallet": 950, "bet": 50}
      ]
    }).to_string();
    
    let _ = self.udp_socket.send_to(game_state.as_bytes(), client_addr);
  }
}

#[test]
fn test_main_flow() {
  let server_addr = "127.0.0.1:10100";
  let server = TestServer::new(server_addr);
  let received_messages = server.run();

  // Start client
  let client = Client::new(server_addr, "Player1".to_string(), 1000)
    .expect("Failed to create client");

  // Simulate join
  client.join_game();

  // Assert that JOIN was received by the server
  let join_msg = received_messages.recv_timeout(Duration::from_secs(2))
    .expect("Did not receive JOIN message");
  assert!(join_msg.starts_with("JOIN"));

  // Spawn UDP listener
  let client_for_udp = client.clone();
  let udp_thread = thread::spawn(move || {
    client_for_udp.listen_for_udp();
  });

  // Send UDP game state update
  thread::sleep(Duration::from_millis(100));
  server.send_game_update(server_addr);

  // Wait briefly for the update to process
  thread::sleep(Duration::from_millis(200));

  // Shutdown listener cleanly
  client.leave_game();
  udp_thread.join().expect("Failed to join UDP listener thread");
}