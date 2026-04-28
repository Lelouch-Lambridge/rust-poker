use poker::card::{Card, encode_card, decode_card};

#[test]
fn test_encode_decode_card() {
  for suit in 0..4 {
    for value in 1..14 {
      let encoded = encode_card(suit, value);
      let (decoded_suit, decoded_value) = decode_card(encoded);
      assert_eq!(suit, decoded_suit);
      assert_eq!(value, decoded_value);
    }
  }
}

#[test]
fn test_card_flip() {
  let mut card = Card::from_suit_value(1, 10);
  assert_eq!(card.face_up(), false);

  card.flip();
  assert_eq!(card.face_up(), true);

  card.flip();
  assert_eq!(card.face_up(), false);
}

#[test]
fn test_card_display() {
  let card = Card::from_suit_value(2, 1);
  assert_eq!(card.to_string(), "♥A");
}
