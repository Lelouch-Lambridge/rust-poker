use simple_logger::SimpleLogger;
use log::LevelFilter;
use poker::games::{fivecarddraw::FiveCardDraw, texasholdem::TexasHoldem, sevencardstud::SevenCardStud};

mod clienthandler;
mod servermanager;
use servermanager::ServerManager;

use bb8::Pool;
use bb8_postgres::PostgresConnectionManager;
use tokio_postgres::NoTls;
use std::sync::Arc;

use server::db::{GameDatabase, DbRepo};
use env_logger;

#[tokio::main]
async fn main() {
  // SimpleLogger::new().with_level(LevelFilter::Warn).init().unwrap();
  env_logger::init();
  let manager = PostgresConnectionManager::new_from_stringlike(
    "host=localhost user=postgres dbname=poker",
    NoTls,
  ).unwrap();

  let pool = Pool::builder().build(manager).await.unwrap();
  let db = Arc::new(DbRepo::new(pool));

  println!("Poker Dealer Server Starting...");
  let server_manager = ServerManager::new(db);

  server_manager.lock().unwrap().register_game::<TexasHoldem>();
  server_manager.lock().unwrap().register_game::<FiveCardDraw>();
  server_manager.lock().unwrap().register_game::<SevenCardStud>();

  server_manager.lock().unwrap().start_lobby();
  server_manager.lock().unwrap().list_games();

  // server_manager.shutdown_game("TexasHoldem");

  loop {
    std::thread::park();
  }
}
