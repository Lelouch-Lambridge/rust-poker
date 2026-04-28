use serde::{Serialize, Deserialize};
use std::ops::{Deref, DerefMut};
use std::fmt;

pub fn encode_card(suit: u8, value: u8) -> u8 { (suit << 4) | value }
pub fn decode_card(encoded: u8) -> (u8, u8) { ((encoded >> 4) & 0x3, encoded & 0xF) }

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Card(pub u8);

impl Deref for Card {
  type Target = u8;
  fn deref(&self) -> &Self::Target { &self.0 }
}
impl DerefMut for Card {
  fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}

impl Card {
  pub fn new() -> Card { Card(0) }
  pub fn from_encoded(card: u8) -> Card { Card(card) }
  pub fn from_suit_value(suit: u8, value: u8) -> Card { Card((suit << 4) | value) }

  pub fn decode(&self) -> (u8, u8) { ((self.0 >> 4) & 0x3, self.0 & 0xF) }
  pub fn decode_f(&self) -> (bool, u8, u8) { (self.0 >> 6 != 0, (self.0 >> 4) & 0x3, self.0 & 0xF) }
  pub fn face_up(&mut self) -> bool { self.0 >> 6 != 0 }
  pub fn flip(&mut self) { self.0 ^= 1 << 6 }
  pub fn flip_up(&mut self) { self.0 |= 1 << 6 }
  pub fn flip_down(&mut self) { self.0 &= 0x3F }
}

impl fmt::Display for Card {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let (suit_index, value_index) = self.decode();

    let suit = match suit_index {
      0 => "♦",
      1 => "♣",
      2 => "♥",
      3 => "♠",
      _ => "?",
    };

    let value = match value_index {
      1 => "A",
      2..=10 => &value_index.to_string(),
      11 => "J",
      12 => "Q",
      13 => "K",
      14 => "A",
      _ => "?", 
    };

    if value == "?" || suit == "?" {
      write!(f, "??")
    } else {
      write!(f, "{}{}", suit, value)
    }
  }
}

#[macro_export] macro_rules! card_new {
  () => { Card::new() }; 
  ($card:expr) => { Card::from_encoded($card) }; 
  ($suit:expr, $value:expr) => { Card::from_suit_value($suit, $value) };
}