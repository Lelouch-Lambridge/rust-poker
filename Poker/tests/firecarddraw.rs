use poker::games::fivecarddraw::FiveCardDraw;
use poker::game::Game;
use poker::player::{Player, PlayerNode};

use std::net::{TcpStream, TcpListener};
use std::sync::{Arc, Mutex};

fn dummy_stream() -> TcpStream {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let addr = listener.local_addr().unwrap();

  std::thread::spawn(move || {
    TcpStream::connect(addr).unwrap();
  });

  listener.accept().unwrap().0
}


fn make_node(player: Player) -> Arc<Mutex<PlayerNode>> {
  Arc::new(Mutex::new(PlayerNode {
    player,
    stream: dummy_stream(),
    next: None,
    prev: None,
    nextf: None,
    prevf: None,
  }))
}

#[test]
fn test_start_and_deal() {
  let mut game = FiveCardDraw::new();
  game.start_game();
  assert_eq!(game.get_deck().len(), 52);

  let mut player = Player::new(1, "Alice".to_string(), 1000);
  game.deal_to_player(&mut player).unwrap();

  assert_eq!(player.hand.len(), 5);
  assert_eq!(game.get_deck().len(), 47); // 52 - 5
}

#[test]
fn test_replace_cards_success() {
  let mut game = FiveCardDraw::new();
  game.start_game();

  let player = Player::new(1, "Bob".to_string(), 1000);
  let node = make_node(player);
  game.deal_to_player(&mut node.lock().unwrap().player).unwrap();

  let hand = &node.lock().unwrap().player.hand.clone();
  let discard = vec![hand[0], hand[1]];

  game.replace_cards(&mut node.clone(), discard).unwrap();
  let new_hand = &node.lock().unwrap().player.hand;
  assert_eq!(new_hand.len(), 5);
}

#[test]
fn test_replace_too_many_cards() {
  let mut game = FiveCardDraw::new();
  game.start_game();

  let player = Player::new(1, "Bob".to_string(), 1000);
  let node = make_node(player);
  game.deal_to_player(&mut node.lock().unwrap().player).unwrap();

  let hand = &node.lock().unwrap().player.hand.clone();
  let discard = hand[..5].to_vec();

  let result = game.replace_cards(&mut node.clone(), discard);
  assert!(result.is_err());
}
