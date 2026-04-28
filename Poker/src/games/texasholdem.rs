use std::fmt;
use std::sync::{Arc, Mutex};
use crate::game::Game;
use crate::hand::Hand;
use crate::{deck::Deck, generate_deck};
use crate::player::{Player, PlayerNode};

pub struct TexasHoldem {
  round: u8,
  deck: Deck,
  community_cards: Hand,
}

impl TexasHoldem {
  pub fn community_len(&self) -> usize {
    self.community_cards.len()
  }

  pub fn set_community_cards(&mut self, hand: Hand) {
    self.community_cards = hand;
  }
}

impl Game for TexasHoldem {
  fn new() -> Self {
    TexasHoldem {
      round: 0,
      deck: Deck(vec![]),
      community_cards: Hand(vec![]),
    }
  }
  
  fn is_ante(&self) -> bool { false }

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
    match self.round {
      0 => {
        // Deal 2 cards to each player in order
        let Some(head_node) = head.clone() else { return false; };
        let mut current = Some(head_node.clone());
        let mut first_iteration = true;
        while let Some(node) = current.take() {
          let mut player_locked = node.lock().unwrap();
          
          let _ = self.deal_to_player(&mut player_locked.player);

          
          current = player_locked.next.clone();
          drop(player_locked);

          if let Some(next_node) = &current {
            if Arc::ptr_eq(next_node, &head_node) && !first_iteration { break; }
          }
          first_iteration = false;
        }
      },
      1 => self.community_cards.extend(self.deck.deal_face_up(3)), // Flop
      2 => self.community_cards.extend(self.deck.deal_face_up(1)), // Turn
      3 => self.community_cards.extend(self.deck.deal_face_up(1)), // River
      _ => return true,
    }
    false
  }
  
  fn deal_to_player(&mut self, player: &mut Player) -> Result<(), String> {
    if player.folded {
      return Ok(());
    }

    let cards = self.deck.deal(2);
    player.hand.extend(cards.into_iter());
    Ok(())
  }
  
  fn start_game(&mut self) {
    self.round = 0;
    self.deck = generate_deck!();
    self.community_cards = Hand::new();
  }

  fn determine_winner(&self, players: &[(&u64, &Hand)]) -> Option<u64> {
    players.iter()
      .max_by(|(_, hand_a), (_, hand_b)| {
        let mut combined_a = hand_a.0.clone();
        combined_a.extend(&self.community_cards.0);

        let mut combined_b = hand_b.0.clone();
        combined_b.extend(&self.community_cards.0);

        Hand(combined_a).evaluate().cmp(&Hand(combined_b).evaluate())
      }).map(|(id, _)| **id)
  }

  fn to_broadcast(&self) -> serde_json::Value {
    serde_json::json!({
      "round": self.round,
      "community_cards": self.community_cards,
    })
  }

  fn showdown_hand(&self, hand: &Hand) -> Hand {
    let mut combined = hand.0.clone();
    combined.extend(&self.community_cards.0);
    Hand(combined)
  }
}

impl fmt::Display for TexasHoldem {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "\n Texas Hold'em:\n  Round: {}\n  Community Cards: {}", self.round, self.community_cards)
  }
}
