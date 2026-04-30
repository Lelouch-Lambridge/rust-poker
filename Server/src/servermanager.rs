use std::collections::HashMap;
use std::net::{TcpListener, TcpStream, UdpSocket, IpAddr};
use socket2::SockRef;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use local_ip_address::local_ip;
use std::any::type_name;
use std::io::Write;
#[allow(unused_imports)]
use log::{error, info, debug};

use crate::clienthandler::{handle_game_client, handle_lobby_client};
use server::Table;
use poker::game::Game;
use server::db::GameDatabase;

pub struct TableInfo {
  port: u16,
  multicast_addr: String,
  shutdown_flag: Arc<AtomicBool>,
  game_running: Arc<AtomicBool>,
  tcp_handle: Option<JoinHandle<()>>,
  broadcaster_handle: Option<JoinHandle<()>>,
  monitor_handle: Option<JoinHandle<()>>,
}

impl TableInfo {
  pub fn get_port(&self) -> u16 {
    self.port
  }

  pub fn get_multicast_addr(&self) -> String {
    self.multicast_addr.clone()
  }
}

pub struct ServerManager<Db: GameDatabase> {
  tables: Arc<Mutex<HashMap<String, TableInfo>>>,
  lobby_handle: Option<JoinHandle<()>>,
  lobby_discovery_handle: Option<JoinHandle<()>>,
  shutdown_flag: Arc<AtomicBool>,
  lobby_clients: Arc<Mutex<Vec<Arc<Mutex<TcpStream>>>>>,
  db: Arc<Db>,
}

impl<Db: GameDatabase + 'static> ServerManager<Db> {
  pub fn new(db: Arc<Db>) -> Arc<Mutex<Self>> {
    let shutdown_flag = Arc::new(AtomicBool::new(false));

    Arc::new(Mutex::new(Self {
      tables: Arc::new(Mutex::new(HashMap::new())),
      lobby_handle: None,
      lobby_discovery_handle: None,
      shutdown_flag,
      lobby_clients: Arc::new(Mutex::new(Vec::new())),
      db,
    }))
  }

  fn clone_shallow(&self) -> Self {
    ServerManager {
      tables: self.tables.clone(),
      lobby_handle: None,
      lobby_discovery_handle: None,
      shutdown_flag: self.shutdown_flag.clone(),
      lobby_clients: self.lobby_clients.clone(),
      db: self.db.clone(),
    }
  }

  pub fn get_tables(&self) -> Arc<Mutex<HashMap<String, TableInfo>>> {
    self.tables.clone()
  }

  pub fn get_lobby(&self) -> Arc<Mutex<Vec<Arc<Mutex<TcpStream>>>>> {
    self.lobby_clients.clone()
  }

  pub fn start_lobby(&mut self) {
    let port = 12345;
    let shutdown_flag = self.shutdown_flag.clone();
    let clients = self.lobby_clients.clone();
    let tables = self.tables.clone();

    let lobby_handle = Some(launch_lobby_server(
      port,
      shutdown_flag.clone(),
      Arc::new(Mutex::new(self.clone_shallow())),
    ));

    let discovery_handle = Some(launch_lobby_discovery_broadcaster(port, shutdown_flag.clone()));

    thread::spawn(move || {
      while !shutdown_flag.load(Ordering::SeqCst) {
        let game_list = {
          let tables = tables.lock().unwrap();
          let names = tables.keys().cloned().collect::<Vec<_>>();
          format!("[GAMES] {{\"games\":[\"{}\"]}}\n", names.join("\",\""))
        };

        let mut clients = clients.lock().unwrap();
        clients.retain(|client| {
          let mut stream = client.lock().unwrap();
          stream.write_all(game_list.as_bytes()).is_ok()
        });

        thread::sleep(std::time::Duration::from_secs(10));
      }
    });

    self.lobby_handle = lobby_handle;
    self.lobby_discovery_handle = discovery_handle;
  }

  fn with_handles(mut self, lobby_handle: Option<JoinHandle<()>>, lobby_discovery_handle: Option<JoinHandle<()>>) -> Self {
    self.lobby_handle = lobby_handle;
    self.lobby_discovery_handle = lobby_discovery_handle;
    self
  }

  pub fn register_game<G: Game + Send + 'static>(&self) {
    let count = self.tables.lock().unwrap().len();
    let port = 10000 + count as u16;
    // TODO Check possibly change UDP multicast address
    let multicast_addr = format!("224.0.0.{}", (count+1) % 256);

    let game_name = Self::type_name_short::<G>();


    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let game_running = Arc::new(AtomicBool::new(false));

    let (tcp_handle, broadcaster_handle, monitor_handle) =
      launch_table::<G, Db>(port, &multicast_addr, shutdown_flag.clone(), game_running.clone(), self.db.clone());

    let info = TableInfo {
      port, multicast_addr: multicast_addr.to_string(),
      shutdown_flag, game_running,
      tcp_handle: Some(tcp_handle),
      broadcaster_handle: Some(broadcaster_handle),
      monitor_handle: Some(monitor_handle),
    };

    println!("Registered game {} on port {} / {}", game_name, port, multicast_addr);
    self.tables.lock().unwrap().insert(game_name, info);
  }

  pub fn shutdown_all(&mut self) {
    self.shutdown_flag.store(true, Ordering::SeqCst);

    // if let Some(handle) = &self.lobby_handle {
    //   let _ = handle.join();
    // }
    // if let Some(handle) = &self.lobby_discovery_handle {
    //   let _ = handle.join();
    // }

    if let Some(handle) = self.lobby_handle.take() {
      let _ = handle.join();
    }
    if let Some(handle) = self.lobby_discovery_handle.take() {
      let _ = handle.join();
    }

    let game_names: Vec<String> = self.tables.lock().unwrap().keys().cloned().collect();
    for name in game_names {
      self.shutdown_game(&name);
    }
  }

  pub fn shutdown_game(&mut self, game_name: &str) {
    let mut tables = self.tables.lock().unwrap();
    let Some(info) = tables.remove(game_name) else {
      println!("Game '{}' not found", game_name);
      return;
    };
    info.shutdown_flag.store(true, Ordering::SeqCst);

    if let Some(handle) = info.tcp_handle { let _ = handle.join(); }
    if let Some(handle) = info.broadcaster_handle { let _ = handle.join(); }

    println!("Shutdown {}", game_name);
  }

  pub fn list_games(&self) -> Vec<String> {
    let tables = self.tables.lock().unwrap();
    tables.keys().cloned().collect()
  }

  fn type_name_short<T: ?Sized>() -> String {
    let full_name = type_name::<T>();
    full_name.split("::").last().unwrap_or(full_name).to_string()
  }
}

fn launch_table<G: Game + Send + 'static, Db: GameDatabase + 'static>(port: u16, multicast_addr: &str, shutdown_flag: Arc<AtomicBool>, game_running: Arc<AtomicBool>, db: Arc<Db>) -> (JoinHandle<()>, JoinHandle<()>, JoinHandle<()>) {
  let tcp_addr = format!("0.0.0.0:{}", port);
  let udp_addr = format!("{}:{}", multicast_addr, port);

  let tcp_listener = TcpListener::bind(&tcp_addr).expect("TCP Bind Failed");
  let table = Arc::new(Mutex::new(Table::<G, Db>::new(&udp_addr, db)));

  let tcp_flag = shutdown_flag.clone();
  let table_tcp = table.clone();
  let tcp_handle = thread::spawn(move || {
    for stream in tcp_listener.incoming() {
      if tcp_flag.load(Ordering::SeqCst) {
        break;
      }

      if let Ok(stream) = stream {
        let table_clone = table_tcp.clone();
        thread::spawn(move || handle_game_client(stream, table_clone));
      }
    }
  });

  let broadcast_flag = shutdown_flag.clone();
  let table_clone = table.clone();
  let broadcaster_handle = thread::spawn(move || {
    while !broadcast_flag.load(Ordering::SeqCst) {
      thread::sleep(std::time::Duration::from_secs(5));
      let table = table_clone.lock().unwrap();
      table.broadcast_state();
    }
  });

  let monitor_flag = shutdown_flag.clone();
  let table_monitor = table.clone();
  let monitor_handle = thread::spawn(move || {
    loop {
      if monitor_flag.load(Ordering::SeqCst) {
        break;
      }

      {
        let mut table = table_monitor.lock().unwrap();
        if table.expire_timed_out_turn().is_some() {
          table.broadcast_state();
        }
      }

      if {
        let table = table_monitor.lock().unwrap();
        table.get_num_players() >= 2 && !table.is_running()
      } {
        const AUTO_WAIT: u64 = 30;
        info!("Auto-start in {} seconds...", AUTO_WAIT);
        thread::sleep(std::time::Duration::from_secs(AUTO_WAIT));

        let mut table = table_monitor.lock().unwrap();
        if table.get_num_players() >= 2 && !table.is_running() {
          info!("Auto-starting game now.");
          table.start_game();
          game_running.store(true, Ordering::SeqCst);
        }
      }

      thread::sleep(std::time::Duration::from_secs(2));
    }
  });

  (tcp_handle, broadcaster_handle, monitor_handle)
}

fn launch_lobby_discovery_broadcaster(port: u16, shutdown_flag: Arc<AtomicBool>) -> JoinHandle<()> {
  thread::spawn(move || {
    let multicast_addr = "239.1.1.1:4242";
    let socket = UdpSocket::bind("0.0.0.0:0").expect("Discovery bind failed");

    socket.set_multicast_loop_v4(true).unwrap();
    socket.set_multicast_ttl_v4(1).unwrap();

    let local_ip = match local_ip() {
      Ok(IpAddr::V4(ipv4)) => ipv4,
      Ok(_) => {
        error!("Expected IPv4 local IP, got IPv6");
        return;
      }
      Err(e) => {
        error!("Failed to get local IP: {}", e);
        return;
      }
    };

    SockRef::from(&socket).set_multicast_if_v4(&local_ip).unwrap();

    while !shutdown_flag.load(Ordering::SeqCst) {
      let message = format!("LOBBY:{}:{}", local_ip, port);
      let _ = socket.send_to(message.as_bytes(), multicast_addr);
      thread::sleep(std::time::Duration::from_secs(2));
    }
  })
}

fn launch_lobby_server<Db: GameDatabase + 'static>(port: u16, shutdown_flag: Arc<AtomicBool>, server_manager: Arc<Mutex<ServerManager<Db>>>) -> JoinHandle<()> {
  thread::spawn(move || {
    let listener = TcpListener::bind(("0.0.0.0", port)).expect("Failed to bind lobby server");

    while !shutdown_flag.load(Ordering::SeqCst) {
      match listener.accept() {
        Ok((stream, _addr)) => {
          let manager = server_manager.clone();
          thread::spawn(move || { handle_lobby_client(stream, manager); });
        }
        Err(_) => continue,
      }
    }
  })
}
