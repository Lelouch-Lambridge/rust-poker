use dealer::table::Table;
use poker::{player::Player, hand::Hand};

#[test]
fn test_table_creation() {
  println!("test_table_creation");
  const UDP_TEST_PORT: &str = "0.0.0.0:8888";
  let table = Table::new(UDP_TEST_PORT);
  assert_eq!(table.players.len(), 0);
  assert!(table.head.is_none());
  assert!(table.tail.is_none());
  assert!(table.blind.is_none());
  assert!(table.turn.is_none());
  assert_eq!(table.river.len(), 0);
  assert_eq!(table.bet, 0);
  assert_eq!(table.pot, 0);
  assert_eq!(table.udp_port, UDP_TEST_PORT);
}

#[test]
fn test_add_player() {
  println!("test_add_player");
  const UDP_TEST_PORT: &str = "0.0.0.0:8888";
  let mut table = Table::new(UDP_TEST_PORT);
  let player = Player {
    id: 1,
    name: "Alice".to_string(),
    wallet: 1000,
    hand: Hand(vec![]),
    hand_size: 0,
    bet: 0,
    folded: false,
  };
  
  table.add_player(player);
  
  assert_eq!(table.players.len(), 1);
  assert!(table.players.contains_key(&1));
  assert!(table.head.is_some());
  assert!(table.tail.is_some());
  
  let head_player = table.head.as_ref().unwrap().lock().unwrap();
  assert_eq!(head_player.player.id, 1);
  assert_eq!(head_player.player.name, "Alice");
  assert_eq!(head_player.player.wallet, 1000);
}

#[test]
fn test_add_multiple_players() {
  println!("test_add_multiple_players");
  const UDP_TEST_PORT: &str = "0.0.0.0:8888";
  let mut table = Table::new(UDP_TEST_PORT);
  
  // Add first player
  let player1 = Player {
    id: 1,
    name: "Alice".to_string(),
    wallet: 1000,
    hand: Hand(vec![]),
    hand_size: 0,
    bet: 0,
    folded: false,
  };
  table.add_player(player1);
  
  // Add second player
  let player2 = Player {
    id: 2,
    name: "Bob".to_string(),
    wallet: 1500,
    hand: Hand(vec![]),
    hand_size: 0,
    bet: 0,
    folded: false,
  };
  table.add_player(player2);
  
  assert_eq!(table.players.len(), 2);
  
  // Check that the linked list is correctly set up
  {
    let head_player = table.head.as_ref().unwrap().lock().unwrap();
    assert_eq!(head_player.player.id, 1);
    assert!(head_player.next.is_some());
    assert!(head_player.prev.is_none());
    
    let next_id = head_player.next.as_ref().unwrap().lock().unwrap().player.id;
    assert_eq!(next_id, 2);
  } // Release the lock before further operations
  
  // Check that the tail points to the second player
  let tail_player = table.tail.as_ref().unwrap().lock().unwrap();
  assert_eq!(tail_player.player.id, 2);
}

#[test]
fn test_get_player() {
  println!("test_get_player");
  const UDP_TEST_PORT: &str = "0.0.0.0:8888";
  let mut table = Table::new(UDP_TEST_PORT);
  
  let player = Player {
    id: 42,
    name: "Charlie".to_string(),
    wallet: 2000,
    hand: Hand(vec![]),
    hand_size: 0,
    bet: 0,
    folded: false,
  };
  
  table.add_player(player);
  
  // Test existing player
  let found_player = table.get_player(42);
  assert!(found_player.is_some());
  assert_eq!(found_player.unwrap().lock().unwrap().player.id, 42);
  
  // Test non-existent player
  let not_found = table.get_player(99);
  assert!(not_found.is_none());
}

#[test]
fn test_remove_player() {
  println!("test_remove_player");
  
  // Helper function to get a unique port
  fn get_unique_port() -> String {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(9000);
    
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("0.0.0.0:{}", port)
  }
  
  let udp_port = get_unique_port();
  let mut table = Table::new(&udp_port);
  
  // Add three players
  for i in 1..=3 {
    table.add_player(Player {
      id: i,
      name: format!("Player{}", i),
      wallet: 1000 * i,
      hand: Hand(vec![]),
      hand_size: 0,
      bet: 0,
      folded: false,
    });
  }
  
  assert_eq!(table.players.len(), 3);
  
  // Remove middle player
  table.remove_player(2);
  
  assert_eq!(table.players.len(), 2);
  assert!(table.players.contains_key(&1));
  assert!(!table.players.contains_key(&2));
  assert!(table.players.contains_key(&3));
  
  // Check that player 1 points to player 3
  {
    let head_player = table.head.as_ref().unwrap().lock().unwrap();
    assert_eq!(head_player.player.id, 1);
    assert!(head_player.next.is_some());
    
    // Get the ID without holding the lock
    let next_id = {
      let next_lock = head_player.next.as_ref().unwrap().lock().unwrap();
      next_lock.player.id
    };
    
    assert_eq!(next_id, 3);
  }
  
  // Check that player 3 points back to player 1
  {
    let tail_player = table.tail.as_ref().unwrap().lock().unwrap();
    assert_eq!(tail_player.player.id, 3);
    assert!(tail_player.prev.is_some());
    
    // Get the ID without holding the lock
    let prev_id = {
      let prev_lock = tail_player.prev.as_ref().unwrap().lock().unwrap();
      prev_lock.player.id
    };
    
    assert_eq!(prev_id, 1);
  }
  
  // Remove first player
  table.remove_player(1);
  
  assert_eq!(table.players.len(), 1);
  assert!(!table.players.contains_key(&1));
  
  // Head should now point to player 3
  {
    let new_head = table.head.as_ref().unwrap().lock().unwrap();
    assert_eq!(new_head.player.id, 3);
    assert!(new_head.prev.is_none());
  }
  
  // Remove last player
  table.remove_player(3);
  
  assert_eq!(table.players.len(), 0);
  assert!(table.head.is_none());
  assert!(table.tail.is_none());
}

#[test]
fn test_deal_cards() {
  println!("test_deal_cards");
  const UDP_TEST_PORT: &str = "0.0.0.0:8888";
  let mut table = Table::new(UDP_TEST_PORT);
  
  let player = Player {
    id: 1,
    name: "Alice".to_string(),
    wallet: 1000,
    hand: Hand(vec![]),
    hand_size: 0,
    bet: 0,
    folded: false,
  };
  
  table.add_player(player);
  
  // Deal 2 cards (standard poker hand)
  let cards = table.deal_cards(1, 2);
  
  assert_eq!(cards.len(), 2);
  
  // Check if the cards were assigned to the player
  let player_node = table.get_player(1).unwrap();
  let player_locked = player_node.lock().unwrap();
  assert_eq!(player_locked.player.hand.len(), 2);
  assert_eq!(player_locked.player.hand_size, 2);
  
  // Dealing to non-existent player should return empty vector
  let cards_nonexistent = table.deal_cards(99, 2);
  assert!(cards_nonexistent.is_empty());
}

#[test]
fn test_raise() {
  println!("test_raise");
  const UDP_TEST_PORT: &str = "0.0.0.0:8888";
  let mut table = Table::new(UDP_TEST_PORT);
  
  let player = Player {
    id: 1,
    name: "Alice".to_string(),
    wallet: 1000,
    hand: Hand(vec![]),
    hand_size: 0,
    bet: 0,
    folded: false,
  };
  
  table.add_player(player);
  
  // Raise by 100
  let (wallet, bet) = table.raise(1, 100);
  
  assert_eq!(wallet, 900);
  assert_eq!(bet, 100);
  assert_eq!(table.pot, 100);
  assert_eq!(table.bet, 100);
  
  // Check player state
  let player_node = table.get_player(1).unwrap();
  let player_locked = player_node.lock().unwrap();
  assert_eq!(player_locked.player.wallet, 900);
  assert_eq!(player_locked.player.bet, 100);
  
  // Attempt to raise for non-existent player
  let (wallet, bet) = table.raise(99, 100);
  assert_eq!(wallet, 0);
  assert_eq!(bet, 0);
}

#[test]
fn test_check() {
  println!("test_check");
  const UDP_TEST_PORT: &str = "0.0.0.0:8888";
  let mut table = Table::new(UDP_TEST_PORT);
  
  // Add two players
  let player1 = Player {
    id: 1,
    name: "Alice".to_string(),
    wallet: 1000,
    hand: Hand(vec![]),
    hand_size: 0,
    bet: 0,
    folded: false,
  };
  
  let player2 = Player {
    id: 2,
    name: "Bob".to_string(),
    wallet: 1000,
    hand: Hand(vec![]),
    hand_size: 0,
    bet: 0,
    folded: false,
  };
  
  table.add_player(player1);
  table.add_player(player2);
  
  // Player 1 raises
  table.raise(1, 50);
  assert_eq!(table.bet, 50);
  
  // Player 2 checks (matches the current bet)
  let (wallet, bet) = table.check(2);
  
  assert_eq!(wallet, 950);
  assert_eq!(bet, 50);
  assert_eq!(table.pot, 100);
  
  // Check state of player 2
  let player_node = table.get_player(2).unwrap();
  let player_locked = player_node.lock().unwrap();
  assert_eq!(player_locked.player.wallet, 950);
  assert_eq!(player_locked.player.bet, 50);
  
  // Check for non-existent player
  let (wallet, bet) = table.check(99);
  assert_eq!(wallet, 0);
  assert_eq!(bet, 0);
}

#[test]
fn test_fold() {
  println!("test_fold");
  const UDP_TEST_PORT: &str = "0.0.0.0:8888";
  let mut table = Table::new(UDP_TEST_PORT);
  
  let player = Player {
    id: 1,
    name: "Alice".to_string(),
    wallet: 1000,
    hand: Hand(vec![]),
    hand_size: 0,
    bet: 0,
    folded: false,
  };
  
  table.add_player(player);
  
  // Player folds
  let folded = table.fold(1);
  
  assert!(folded);
  
  // Check player state
  let player_node = table.get_player(1).unwrap();
  let player_locked = player_node.lock().unwrap();
  assert!(player_locked.player.folded);
  
  // Fold for non-existent player
  let folded = table.fold(99);
  assert!(!folded);
}

#[test]
fn test_start_game_and_advance_turn() {
  println!("test_start_game_and_advance_turn");
  
  // Helper function to get a unique port
  fn get_unique_port() -> String {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(9000);
    
    let port = PORT_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("0.0.0.0:{}", port)
  }
  
  let udp_port = get_unique_port();
  let mut table = Table::new(&udp_port);
  
  // Add three players
  for i in 1..=3 {
    table.add_player(Player {
      id: i,
      name: format!("Player{}", i),
      wallet: 1000,
      hand: Hand(vec![]),
      hand_size: 0,
      bet: 0,
      folded: false,
    });
  }
  
  // Start game
  table.start_game();
  
  // Check that turn is set to first player
  assert!(table.turn.is_some());
  {
    let turn_player_id = table.turn.as_ref().unwrap().lock().unwrap().player.id;
    assert_eq!(turn_player_id, 1);
  }
  
  // Advance turn
  table.advance_turn();
  
  // Turn should now be player 2
  {
    let turn_player_id = table.turn.as_ref().unwrap().lock().unwrap().player.id;
    assert_eq!(turn_player_id, 2);
  }
  
  // Fold player 2 and advance
  table.fold(2);
  table.advance_turn();
  
  // Turn should now be player 3
  {
    let turn_player_id = table.turn.as_ref().unwrap().lock().unwrap().player.id;
    assert_eq!(turn_player_id, 3);
  }
  
  // Advance again, should wrap around to player 1 (skipping folded player 2)
  table.advance_turn();
  {
    let turn_player_id = table.turn.as_ref().unwrap().lock().unwrap().player.id;
    assert_eq!(turn_player_id, 1);
  }
}