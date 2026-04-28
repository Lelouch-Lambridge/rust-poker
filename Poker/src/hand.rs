use serde::{Serialize, Deserialize};
use std::ops::{Deref, DerefMut};
use std::fmt;
use std::collections::HashMap;
use std::cmp::Ordering;
use crate::card::Card;

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum HandRank {
  NoCards(),
  HighCard(Vec<u8>, Vec<u8>),
  OnePair(u8, Vec<u8>, Vec<u8>),
  TwoPair(u8, u8, Vec<u8>, Vec<u8>),
  ThreeOfAKind(u8, Vec<u8>, Vec<u8>),
  Straight(u8, Vec<u8>),
  Flush(Vec<u8>, Vec<u8>),
  FullHouse(u8, u8, Vec<u8>),
  FourOfAKind(u8, Vec<u8>, Vec<u8>),
  StraightFlush(u8, Vec<u8>),
  RoyalFlush(Vec<u8>),
}

impl HandRank {
  pub fn rank_value(&self) -> u8 {
    match self {
      HandRank::NoCards() => 0,
      HandRank::HighCard(_, _) => 1,
      HandRank::OnePair(_, _, _) => 2,
      HandRank::TwoPair(_, _, _, _) => 3,
      HandRank::ThreeOfAKind(_, _, _) => 4,
      HandRank::Straight(_, _) => 5,
      HandRank::Flush(_, _) => 6,
      HandRank::FullHouse(_, _, _) => 7,
      HandRank::FourOfAKind(_, _, _) => 8,
      HandRank::StraightFlush(_, _) => 9,
      HandRank::RoyalFlush(_) => 10,
    }
  }

  pub fn compare_values(a: &[u8], b: &[u8]) -> Ordering {
    a.iter().rev().cmp(b.iter().rev())
  }
}

impl Ord for HandRank {
  fn cmp(&self, other: &Self) -> Ordering {
    let self_rank = self.rank_value();
    let other_rank = other.rank_value();
    self_rank.cmp(&other_rank).then_with(|| {
      match (self, other) {
        (HandRank::NoCards(), HandRank::NoCards()) => Ordering::Equal,
        (HandRank::HighCard(a_v, a_cards), HandRank::HighCard(b_v, b_cards)) => 
          HandRank::compare_values(a_v, b_v).then(a_cards.cmp(b_cards)),
        (HandRank::OnePair(v_a, k_a, a_cards), HandRank::OnePair(v_b, k_b, b_cards)) => 
          v_a.cmp(v_b)
            .then(HandRank::compare_values(k_a, k_b))
            .then(a_cards.cmp(b_cards)),
        (HandRank::TwoPair(a1, a2, k_a, a_cards), HandRank::TwoPair(b1, b2, k_b, b_cards)) => 
          a1.cmp(b1).then(a2.cmp(b2))
            .then(HandRank::compare_values(k_a, k_b))
            .then(a_cards.cmp(b_cards)),
        (HandRank::ThreeOfAKind(v_a, k_a, a_cards), HandRank::ThreeOfAKind(v_b, k_b, b_cards)) => 
          v_a.cmp(v_b)
            .then(HandRank::compare_values(k_a, k_b))
            .then(a_cards.cmp(b_cards)),
        (HandRank::Straight(v_a, a_cards), HandRank::Straight(v_b, b_cards)) => 
          v_a.cmp(v_b).then(a_cards.cmp(b_cards)),
        (HandRank::Flush(a_v, a_cards), HandRank::Flush(b_v, b_cards)) => 
          HandRank::compare_values(a_v, b_v).then(a_cards.cmp(b_cards)),
        (HandRank::FullHouse(t_a, p_a, a_cards), HandRank::FullHouse(t_b, p_b, b_cards)) => 
          t_a.cmp(t_b).then(p_a.cmp(p_b)).then(a_cards.cmp(b_cards)),
        (HandRank::FourOfAKind(v_a, k_a, a_cards), HandRank::FourOfAKind(v_b, k_b, b_cards)) => 
          v_a.cmp(v_b)
            .then(HandRank::compare_values(k_a, k_b))
            .then(a_cards.cmp(b_cards)),
        (HandRank::StraightFlush(v_a, a_cards), HandRank::StraightFlush(v_b, b_cards)) => 
          v_a.cmp(v_b).then(a_cards.cmp(b_cards)),
        (HandRank::RoyalFlush(a_cards), HandRank::RoyalFlush(b_cards)) => 
          a_cards.cmp(b_cards),
        _ => panic!("Mismatched hand ranks during comparison"),
      }
    })
  }
}

impl PartialOrd for HandRank {
 fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> { Some(self.cmp(other)) }
}

impl fmt::Display for HandRank {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    fn value_to_str(value: u8) -> &'static str {
      match value {
        1 => "A", 2 => "2", 3 => "3", 4 => "4", 5 => "5", 
        6 => "6", 7 => "7", 8 => "8", 9 => "9", 10 => "10",
        11 => "J", 12 => "Q", 13 => "K", 14 => "A", _ => "?",
      }
    }

    match self {
      HandRank::RoyalFlush(_) => write!(f, "Royal Flush"),
      HandRank::StraightFlush(high, _) => write!(f, "Straight Flush (High Card {})", value_to_str(*high)),
      HandRank::FourOfAKind(value, kickers, _) => {
        let mut sorted_kickers = kickers.clone();
        sorted_kickers.sort_by(|a, b| b.cmp(a));
        write!(
          f,
          "Four of a Kind ({}) with kickers {}",
          value_to_str(*value),
          sorted_kickers.iter().map(|&v| value_to_str(v)).collect::<Vec<_>>().join(", ")
        )
      }
      HandRank::FullHouse(three, two, _) => write!(
        f,
        "Full House ({}) over ({})",
        value_to_str(*three),
        value_to_str(*two)
      ),
      HandRank::Flush(cards, _) => {
        let mut sorted = cards.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        write!(
          f,
          "Flush ({})",
          sorted.iter().map(|&v| value_to_str(v)).collect::<Vec<_>>().join(", ")
        )
      }
      HandRank::Straight(high, _) => write!(f, "Straight (High Card {})", value_to_str(*high)),
      HandRank::ThreeOfAKind(value, kickers, _) => {
        let mut sorted_kickers = kickers.clone();
        sorted_kickers.sort_by(|a, b| b.cmp(a));
        write!(
          f,
          "Three of a Kind ({}) with kickers {}",
          value_to_str(*value),
          sorted_kickers.iter().map(|&v| value_to_str(v)).collect::<Vec<_>>().join(", ")
        )
      }
      HandRank::TwoPair(p1, p2, kickers, _) => {
        let (high_pair, low_pair) = if p1 > p2 { (*p1, *p2) } else { (*p2, *p1) };
        let mut sorted_kickers = kickers.clone();
        sorted_kickers.sort_by(|a, b| b.cmp(a));
        write!(
          f,
          "Two Pair ({}) and ({}) with kickers {}",
          value_to_str(high_pair),
          value_to_str(low_pair),
          sorted_kickers.iter().map(|&v| value_to_str(v)).collect::<Vec<_>>().join(", ")
        )
      }
      HandRank::OnePair(value, kickers, _) => {
        let mut sorted_kickers = kickers.clone();
        sorted_kickers.sort_by(|a, b| b.cmp(a));
        write!(
          f,
          "One Pair ({}) with kickers {}",
          value_to_str(*value),
          sorted_kickers.iter().map(|&v| value_to_str(v)).collect::<Vec<_>>().join(", ")
        )
      }
      HandRank::HighCard(cards, _) => {
        let mut sorted = cards.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        write!(
          f,
          "High Card ({})",
          sorted.iter().map(|&v| value_to_str(v)).collect::<Vec<_>>().join(", ")
        )
      }
      HandRank::NoCards() => write!(f, "Empty Hand")
    }
  }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Hand(pub Vec<u8>);

impl Deref for Hand {
  type Target = Vec<u8>;
  fn deref(&self) -> &Self::Target { &self.0 }
}
impl DerefMut for Hand {
  fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}

impl Hand {
  pub fn new() -> Self {
    Hand(vec![])
  }

  pub fn evaluate(&self) -> HandRank {
    if self.is_empty() { return HandRank::NoCards(); }

    let (suits, values): (Vec<_>, Vec<_>) = self.iter()
      .map(|c| {
        let (suit, value) = Card(*c).decode();
        (suit, value)
      }).unzip();

    let mut sorted_cards = self.0.clone();
    sorted_cards.sort_by(|a, b| {
      let (suit_a, val_a) = Card(*a).decode();
      let (suit_b, val_b) = Card(*b).decode();
      val_b.cmp(&val_a).then_with(|| suit_b.cmp(&suit_a))
    });

    let is_flush = suits.windows(2).all(|w| w[0] == w[1]);
    let straight_high = Hand::check_straight(&values);
    let value_counts = Hand::count_values(&values);
    let mut count_vec: Vec<_> = value_counts.iter().map(|(&v, &c)| (c, v)).collect();
    count_vec.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));

    // Royal Flush
    if is_flush && straight_high == Some(14) {
      return HandRank::RoyalFlush(sorted_cards);
    }

    // Straight Flush
    if is_flush && straight_high.is_some() {
      return HandRank::StraightFlush(straight_high.unwrap(), sorted_cards);
    }

    // Four of a Kind
    if let Some(&(4, quad_value)) = count_vec.first() {
      let adjusted_quad = if quad_value == 1 { 14 } else { quad_value };
      let kickers: Vec<u8> = values.iter().filter(|&&v| v != quad_value).cloned().collect();
      return HandRank::FourOfAKind(adjusted_quad, kickers, sorted_cards);
    }

    // Full House
    if let Some((&(3, three_val), &(2, pair_val))) = count_vec.first().zip(count_vec.get(1)) {
      let adjusted_three = if three_val == 1 { 14 } else { three_val };
      let adjusted_pair = if pair_val == 1 { 14 } else { pair_val };
      return HandRank::FullHouse(adjusted_three, adjusted_pair, sorted_cards);
    }

    // Flush
    if is_flush {
      let mut sorted_values = values.clone();
      sorted_values.sort_by(|a, b| b.cmp(a));
      return HandRank::Flush(sorted_values, sorted_cards);
    }

    // Straight
    if let Some(high) = straight_high {
      return HandRank::Straight(high, sorted_cards);
    }

    // Three of a Kind
    if let Some(&(3, three_val)) = count_vec.first() {
      let adjusted_val = if three_val == 1 { 14 } else { three_val };
      let kickers: Vec<u8> = values.iter().filter(|&&v| v != three_val).cloned().collect();
      return HandRank::ThreeOfAKind(adjusted_val, kickers, sorted_cards);
    }

    // Two Pair
    if let Some((&(2, pair1), &(2, pair2))) = count_vec.first().zip(count_vec.get(1)) {
      let adjusted_p1 = if pair1 == 1 { 14 } else { pair1 };
      let adjusted_p2 = if pair2 == 1 { 14 } else { pair2 };
      let (higher_pair, lower_pair) = if adjusted_p1 > adjusted_p2 {
      (adjusted_p1, adjusted_p2)
      } else {
      (adjusted_p2, adjusted_p1)
      };
      let kickers = values.iter()
      .filter(|&&v| v != pair1 && v != pair2)
      .cloned()
      .collect();
      return HandRank::TwoPair(higher_pair, lower_pair, kickers, sorted_cards);
    }

    // One Pair
    if let Some(&(2, pair_val)) = count_vec.first() {
      let adjusted_val = if pair_val == 1 { 14 } else { pair_val };
      let kickers: Vec<u8> = values.iter().filter(|&&v| v != pair_val).cloned().collect();
      return HandRank::OnePair(adjusted_val, kickers, sorted_cards);
    }

    // High Card
    let mut sorted_values = values.clone();
    sorted_values.sort_by(|a, b| b.cmp(a));
    HandRank::HighCard(sorted_values, sorted_cards)
  }

  pub fn to_broadcast(&self) -> Hand {
    let face_up_cards: Vec<u8> = self.0.iter()
      .map(|&c| if Card(c).face_up() { c } else { 0 })
      .collect();
    Hand(face_up_cards)
  }

  fn check_straight(values: &[u8]) -> Option<u8> {
    let mut sorted = values.to_vec();
    sorted.sort();
    sorted.reverse();
    
    if sorted == vec![5, 4, 3, 2, 1] { return Some(5); }
    
    let modified: Vec<u8> = sorted.iter().map(|&v| if v == 1 { 14 } else { v }).collect();
    let mut unique = modified.clone();
    unique.sort();
    unique.reverse();
    unique.dedup();
    
    for i in 0..unique.len().saturating_sub(4) {
      let window = &unique[i..i+5];
      if window[0] - window[4] == 4 { return Some(window[0]); }
    }

    None
  }

  pub fn count_values(values: &[u8]) -> HashMap<u8, usize> {
    let mut counts = HashMap::new();
    for &v in values {  *counts.entry(v).or_insert(0) += 1;  }
    counts
  }
}

impl fmt::Display for Hand {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    let cards_str: Vec<String> = self.0.iter()
      .map(|&encoded| Card(encoded).to_string()).collect();
    write!(f, "{}", cards_str.join(", "))
  }
}