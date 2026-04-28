use std::fmt;
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use serde::{Serialize, Deserialize};
use crate::hand::Hand;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Player {
  pub id: u64,
  pub name: String,
  pub wallet: u64,
  pub hand: Hand,
  pub bet: u64,
  pub folded: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct BroadcastPlayer {
  pub id: u64,
  pub name: String,
  pub wallet: u64,
  pub hand: Hand,
  pub bet: u64,
  pub folded: bool,
}

#[derive(Debug)]
pub struct PlayerNode {
  pub player: Player,
  pub stream: Option<TcpStream>,
  pub nextf: Option<Arc<Mutex<PlayerNode>>>,
  pub prevf: Option<Arc<Mutex<PlayerNode>>>,
  pub next: Option<Arc<Mutex<PlayerNode>>>,
  pub prev: Option<Arc<Mutex<PlayerNode>>>,
}

impl Player {
  pub fn new(id: u64, name: String, wallet: u64) -> Self {
    Self {
      id,
      name,
      wallet,
      hand: Hand(Vec::new()),
      bet: 0,
      folded: true,
    }
  }

  pub fn reset(&mut self) {
    self.hand = Hand(Vec::new());
    self.bet = 0;
    self.folded = true;
  }

  pub fn to_stream(&self) -> BroadcastPlayer {
    BroadcastPlayer {
      id: self.id,
      name: self.name.clone(),
      wallet: self.wallet,
      hand: self.hand.clone(),
      bet: self.bet,
      folded: self.folded,
    }
  }

  pub fn to_broadcast(&self) -> BroadcastPlayer {
    BroadcastPlayer {
      id: self.id,
      name: self.name.clone(),
      wallet: self.wallet,
      hand: self.hand.to_broadcast(),
      bet: self.bet,
      folded: self.folded,
    }
  }
}

impl fmt::Display for Player {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(
      f,
      "Player {} ({}): 💰 ${} | Current Bet: ${} | Cards: {}",
      self.id, self.name, self.wallet, self.bet, self.hand
    )
  }
}
