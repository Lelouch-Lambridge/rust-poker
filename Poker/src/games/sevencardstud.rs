use std::fmt;
use std::sync::{Arc, Mutex};
use crate::game::Game;
use crate::hand::Hand;
use crate::{deck::Deck, generate_deck};
use crate::player::{Player, PlayerNode};

pub struct SevenCardStud {
  round: u8,
  deck: Deck,
}

impl SevenCardStud {
}

impl Game for SevenCardStud {
  fn new() -> Self {
    SevenCardStud {
      round: 0,
      deck: Deck(vec![]),
    }
  }
  
  fn is_ante(&self) -> bool { true }

  fn get_round(&self) -> u8 {
    self.round
  }
  
  fn advance_round(&mut self) {
    self.round += 1;
  }
  
  fn get_deck(&self) -> Deck {
    self.deck.clone()
  }

  fn client_action(&mut self, _node: &mut Arc<Mutex<PlayerNode>>, _parts: &[&str]) -> Result<(), &str> { Err("[BET]") }

  fn handle_round_action(&mut self, head: &mut Option<Arc<Mutex<PlayerNode>>>) -> bool {
    if 4 < self.round { return true; }

    let Some(head_node) = head.clone() else { return false; };
    let mut current = Some(head_node.clone());
    let mut first_iteration = true;
    while let Some(node) = current.take() {
      let mut player_locked = node.lock().unwrap();
      
      let _ = self.deal_to_player(&mut player_locked.player);

      current = player_locked.nextf.clone();
      drop(player_locked);

      if let Some(next_node) = &current {
        if Arc::ptr_eq(next_node, &head_node) && !first_iteration { break; }
      }
      first_iteration = false;
    }

    false
  }

  fn deal_to_player(&mut self, player: &mut Player) -> Result<(), String> {
    let (down,up) = match self.round {
      0 => (2, 1),
      1..=3 => (0, 1),
      4 => (1, 0),
      _ => (0, 0),
    };

    let cards = self.deck.deal(down);
    player.hand.extend(cards.into_iter());
    let cards = self.deck.deal_face_up(up);
    player.hand.extend(cards.into_iter());
    Ok(())
  }

  fn start_game(&mut self) {
    self.round = 0;
    self.deck = generate_deck!();
  }

  fn determine_winner(&self, players: &[(&u64, &Hand)]) -> Option<u64> {
    players.iter()
      .max_by(|(_, hand_a), (_, hand_b)| {
        Hand(hand_a.0.clone()).evaluate().cmp(&Hand(hand_b.0.clone()).evaluate())
      }).map(|(id, _)| **id)
  }

  fn to_broadcast(&self) -> serde_json::Value {
    serde_json::json!({
      "round": self.round,
    })
  }
}

impl fmt::Display for SevenCardStud {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "\n Seven Card Stud:\n  Round: {}\n", self.round)
  }
}
