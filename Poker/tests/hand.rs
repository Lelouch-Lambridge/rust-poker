use poker::hand::{Hand, HandRank};
use poker::card::Card;

fn card(suit: u8, value: u8) -> u8 {
  Card::from_suit_value(suit, value).0
}

#[test]
fn test_hand_high_card() {
  let cards = vec![card(0, 2), card(1, 4), card(2, 6)];
  let hand = Hand(cards.clone());
  let rank = hand.evaluate();
  assert!(matches!(rank, HandRank::HighCard(_, _)));
}

#[test]
fn test_hand_one_pair() {
  let cards = vec![card(0, 2), card(1, 2), card(2, 5)];
  let hand = Hand(cards);
  let rank = hand.evaluate();
  assert!(matches!(rank, HandRank::OnePair(_, _, _)));
}

#[test]
fn test_hand_empty() {
  let hand = Hand::new();
  let rank = hand.evaluate();
  assert_eq!(rank, HandRank::NoCards());
}

#[test]
fn test_to_broadcast_face_up_only() {
  let cards = vec![card(1, 10), card(2, 11)];
  let mut hand = Hand(cards.clone());

  hand[0] |= 1 << 6;

  let broadcast = hand.to_broadcast();
  assert_eq!(broadcast[0], hand[0]);
  assert_eq!(broadcast[1], 0);
}

#[test]
fn test_cards_used_against_two_pair_only_needed_kickers() {
  let first = Hand(vec![
    card(0, 2), card(1, 2),
    card(0, 3), card(1, 3),
    card(0, 13), card(0, 12), card(0, 7),
  ]);
  let second = Hand(vec![
    card(2, 2), card(3, 2),
    card(2, 3), card(3, 3),
    card(1, 13), card(1, 10), card(1, 8),
  ]);

  let first_rank = first.evaluate();
  let second_rank = second.evaluate();

  let first_used = first_rank.cards_used_against(&first.0, &[second_rank.clone()]);
  let second_used = second_rank.cards_used_against(&second.0, &[first_rank]);

  assert!(first_used.contains(&card(0, 13)));
  assert!(first_used.contains(&card(0, 12)));
  assert!(!first_used.contains(&card(0, 7)));

  assert!(second_used.contains(&card(1, 13)));
  assert!(second_used.contains(&card(1, 10)));
  assert!(!second_used.contains(&card(1, 8)));
}
