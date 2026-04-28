use std::env;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
#[allow(unused_imports)]
use log::{LevelFilter, debug, info, warn, error, trace};
use simple_logger::SimpleLogger;

use client::{Client, discover_lobby};

fn main() -> std::io::Result<()> {
  // SimpleLogger::new().with_level(LevelFilter::Debug).init().unwrap();
  use env_logger;
  let multicast_ip_str = "239.1.1.1".to_string();
  let lobby_addr = env::args().nth(1).unwrap_or_else(|| {
    discover_lobby(&multicast_ip_str, 4242).unwrap_or_else(|| {
      error!("Could not discover or receive lobby address.");
      std::process::exit(1);
    })
  });

  let multicast_ip = multicast_ip_str.parse::<Ipv4Addr>().expect("Invalid multicast IP");
  let name = "Player".to_string();
  let wallet = 1000;

  let client = Arc::new(Client::new(&lobby_addr, name, wallet, multicast_ip)?);
  client.start_listeners();
  
  client.join();
  info!("Connected to lobby successfully");

  loop {
    println!("\n=== LOBBY MENU ===");
    
    match client.take_lobby_input() {
      Some((tcp_target, multicast_str)) => {
        if tcp_target == "EXIT" && multicast_str == "EXIT" {
          info!("Exiting program...");
          client.stop_listeners()?;
          break;
        }
        
        info!("Joining game at {} with multicast {}", tcp_target, multicast_str);
        
        info!("Leaving current game/lobby...");
        client.leave_game();
        
        client.stop_listeners()?;
        thread::sleep(Duration::from_millis(500));
        
        match client.reconnect_to(tcp_target.clone(), multicast_str.clone()) {
          Ok(_) => {
            info!("Reconnection to game successful");
            client.start_listeners();
            thread::sleep(Duration::from_millis(500));
            
            client.join();
            client.take_input();
            info!("Left game.");
            
            client.leave_game();
            client.stop_listeners()?;
            println!("Returning to lobby...\n");
          },
          Err(e) => {
            error!("Reconnection to game failed: {}", e);
            thread::sleep(Duration::from_secs(2));
          }
        }
        
        match client.reconnect_to(lobby_addr.clone(), multicast_ip_str.clone()) {
          Ok(_) => {
            info!("Reconnection to lobby successful");
            client.start_listeners();
            thread::sleep(Duration::from_millis(500));
            client.join();
          },
          Err(e) => {
            error!("Reconnection to lobby failed: {}", e);
            thread::sleep(Duration::from_secs(2));
          }
        }
      }
      None => {
        info!("No game selected or error in selection");
        thread::sleep(Duration::from_secs(2));
      }
    }
  }
  
  info!("Program terminated.");
  Ok(())
}
