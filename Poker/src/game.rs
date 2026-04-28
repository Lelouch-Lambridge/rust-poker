use std::fmt::Display;
use std::sync::{Arc, Mutex};
use crate::deck::Deck;
use crate::player::{Player, PlayerNode};
use crate::hand::Hand;

pub trait Game: Display {
  fn new() -> Self;

  fn is_ante(&self) -> bool;

  fn get_round(&self) -> u8;

  fn advance_round(&mut self);

  fn get_deck(&self) -> Deck;

  fn client_action(&mut self, node: &mut Arc<Mutex<PlayerNode>>, parts: &[&str]) -> Result<(), &str>;

  fn handle_round_action(&mut self, head: &mut Option<Arc<Mutex<PlayerNode>>>) -> bool;

  fn deal_to_player(&mut self, player: &mut Player) -> Result<(), String>;

  fn start_game(&mut self);

  fn determine_winner(&self, players: &[(&u64, &Hand)]) -> Option<u64>;

  fn to_broadcast(&self) -> serde_json::Value;
}