use std::fmt;
use std::sync::{Arc, Mutex};
use crate::game::Game;
use crate::hand::Hand;
use crate::card::Card;
use crate::{deck::Deck, generate_deck};
use crate::player::{Player, PlayerNode};

pub struct FiveCardDraw {
  round: u8,
  deck: Deck,
}

impl FiveCardDraw {
  pub fn replace_cards(&mut self, node: &mut Arc<Mutex<PlayerNode>>, discard: Vec<u8>) -> Result<(), &str> {
    // println!("Replacing Cards…");
    let mut player_locked = node.lock().map_err(|_| "Lock failed")?;
    
    if 4 < discard.len() { return Err("[ERROR] Discard less cards!!"); }

    let hand = &mut player_locked.player.hand.0;
    let mut temp_hand = hand.clone();
    for card in &discard {
      let pos = temp_hand.iter()
      .position(|c| c == card)
      .ok_or("Card not in hand")?;
      temp_hand.remove(pos);
    }

    if discard.len() == 4 {
      let (_, value) = Card(temp_hand[0]).decode();
      if value != 1 {
        return Err("[ERROR] Can only discard 4 if last card is an ace!!");
      }
    }

    *hand = temp_hand;
    
    let new_cards = self.deck.deal(discard.len());
    hand.extend(new_cards);
    drop(player_locked);
    Ok(())
  }
}

impl Game for FiveCardDraw {
  fn new() -> Self {
    FiveCardDraw {
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

  fn client_action(&mut self, node: &mut Arc<Mutex<PlayerNode>>, parts: &[&str]) -> Result<(), &str> { 
    if self.round != 1 { return Err("[BET]"); }
    match parts {
      ["REPLACE_CARDS", rest @ ..] => {
        let discard_cards: Result<Vec<u8>, _> = rest.iter().map(|s| s.parse()).collect();
        let discard_cards = match discard_cards {
          Ok(cards) => cards,
          Err(_) => {
            return Err("[ERROR] INVALID CARD\n");
          }
        };

        return self.replace_cards(node, discard_cards);
      }
      _ => Err("[ERROR] REPLACE CARDS\n"), 
    }
  }
  
  fn handle_round_action(&mut self, head: &mut Option<Arc<Mutex<PlayerNode>>>) -> bool {
    if 0 == self.round {
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
    } else if 2 < self.round {
      return true
    }
    false
  }
  
  fn deal_to_player(&mut self, player: &mut Player) -> Result<(), String> {
    if player.folded {
      return Ok(());
    }

    let cards = self.deck.deal(5);
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

impl fmt::Display for FiveCardDraw {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "\n Five Card Draw:\n  Round: {}", self.round)
  }
}
