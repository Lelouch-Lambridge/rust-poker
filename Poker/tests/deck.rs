use poker::deck::Deck;
use poker::card::Card;

#[test]
fn test_deck_creation() {
  let deck = Deck::new(false, 1);
  assert_eq!(deck.len(), 52);
}

#[test]
fn test_deck_shuffling() {
  let unshuffled = Deck::new(false, 1);
  let shuffled = Deck::new(true, 1);
  assert_ne!(unshuffled[..], shuffled[..], "Deck should be shuffled");
}

#[test]
fn test_deal_cards() {
  let mut deck = Deck::new(false, 1);
  let dealt = deck.deal(5);
  assert_eq!(dealt.len(), 5);
  assert_eq!(deck.len(), 47);
}

#[test]
fn test_deal_face_up() {
  let mut deck = Deck::new(false, 1);
  let dealt = deck.deal_face_up(3);
  assert_eq!(dealt.len(), 3);
  for card in dealt {
    assert!(Card(card).face_up());
  }
}
