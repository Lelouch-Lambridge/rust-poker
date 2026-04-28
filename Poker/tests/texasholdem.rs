use poker::games::texasholdem::TexasHoldem;
use poker::game::Game;
use poker::player::Player;
use poker::hand::Hand;

#[test]
fn test_community_card_progression() {
  let mut game = TexasHoldem::new();
  game.start_game();
  assert_eq!(game.community_len(), 0);

  game.advance_round();
  game.handle_round_action(&mut None); // Flop
  assert_eq!(game.community_len(), 3);
  
  game.advance_round();
  game.handle_round_action(&mut None); // Turn
  assert_eq!(game.community_len(), 4);
  
  game.advance_round();
  game.handle_round_action(&mut None); // River
  assert_eq!(game.community_len(), 5);
}

#[test]
fn test_determine_winner_combines_community_cards() {
  let mut game = TexasHoldem::new();
  game.start_game();

  game.set_community_cards(Hand(vec![
    0x2E, // ♥10
    0x2B, // ♥11
    0x2C, // ♥12
    0x2D, // ♥13
    0x21, // ♥1 (Ace)
  ]));

  let player1 = Player {
    id: 1,
    name: "P1".into(),
    wallet: 100,
    hand: Hand(vec![0x10, 0x11]), // Low unrelated cards
    bet: 0,
    folded: false,
  };

  let player2 = Player {
    id: 2,
    name: "P2".into(),
    wallet: 100,
    hand: Hand(vec![0x22, 0x23]), // ♥2, ♥3 — completes royal flush
    bet: 0,
    folded: false,
  };

  let winner = game.determine_winner(&[
    (&player1.id, &player1.hand),
    (&player2.id, &player2.hand),
  ]);

  assert_eq!(winner, Some(2));
}
