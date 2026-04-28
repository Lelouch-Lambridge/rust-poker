use poker::player::Player;
use poker::hand::Hand;
use poker::card::Card;

#[test]
fn test_player_new() {
  let player = Player::new(42, "Alice".into(), 500);
  assert_eq!(player.id, 42);
  assert_eq!(player.name, "Alice");
  assert_eq!(player.wallet, 500);
  assert!(player.hand.is_empty());
  assert!(player.folded);
}

#[test]
fn test_player_reset() {
  let mut player = Player::new(1, "Bob".into(), 1000);
  player.bet = 300;
  player.folded = false;
  player.hand = Hand(vec![10, 20]);
  player.reset();

  assert_eq!(player.bet, 0);
  assert!(player.hand.is_empty());
  assert!(player.folded);
}

#[test]
fn test_to_stream_and_broadcast() {
  let mut player = Player::new(2, "Charlie".into(), 100);
  player.hand = Hand(vec![Card::from_suit_value(0, 5).0 | (1 << 6)]);

  let stream = player.to_stream();
  let broadcast = player.to_broadcast();

  assert_eq!(stream.hand[0], player.hand[0]);
  assert_eq!(broadcast.hand[0], player.hand[0]);

  player.hand[0] &= 0x3F;
  let broadcast_hidden = player.to_broadcast();
  assert_eq!(broadcast_hidden.hand[0], 0);
}
