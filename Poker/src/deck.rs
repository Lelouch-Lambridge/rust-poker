use serde::{Serialize, Deserialize};
use rand::seq::SliceRandom;
use rand::thread_rng;
use std::ops::{Deref, DerefMut};
use std::fmt;
use crate::{card::Card, card_new};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Deck(pub Vec<u8>);

impl Deref for Deck {
  type Target = Vec<u8>;
  fn deref(&self) -> &Self::Target { &self.0 }
}
impl DerefMut for Deck {
  fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}

impl Deck {
  pub fn new(shuffle: bool, num_decks: usize) -> Self {
    let mut decks: Vec<u8> = (0..num_decks).flat_map(|_| {
      (0..4).flat_map(move |suit| 
      (1..14).map(move |value| *card_new!(suit, value)))
      }).collect();
    
    if shuffle { decks.shuffle(&mut thread_rng()); }
    
    Deck(decks)
  }

  pub fn deal(&mut self, num_deal: usize) -> Vec<u8> {
    let len = self.len();
    self.split_off(len - num_deal.min(len))
  }

  pub fn deal_face_up(&mut self, num_deal: usize) -> Vec<u8> {
    let dealt = self.deal(num_deal).into_iter()
      .map(|card| card | (1 << 6)).collect();
    dealt
  }
}

impl fmt::Display for Deck {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let cards_str: Vec<String> = self.0.iter()
      .map(|&encoded| Card(encoded).to_string()).collect();
    write!(f, "{}", cards_str.join(", "))
  }
}

#[macro_export] macro_rules! generate_deck {
  () => { Deck::new(true, 1) };
  ($shuffle:expr) => { Deck::new($shuffle, 1) };
  ($shuffle:expr, $num_decks:expr) => { Deck::new($shuffle, $num_decks) };
}
