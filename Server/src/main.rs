#[allow(dead_code)]
mod clienthandler;
#[allow(dead_code)]
mod servermanager;
mod web;

use bb8::Pool;
use bb8_postgres::PostgresConnectionManager;
use tokio_postgres::NoTls;
use std::sync::Arc;

use server::db::{GameDatabase, DbRepo};

#[tokio::main]
async fn main() {
  env_logger::init();
  let manager = PostgresConnectionManager::new_from_stringlike(
    "host=localhost user=postgres dbname=poker",
    NoTls,
  ).unwrap();

  let pool = Pool::builder().build(manager).await.unwrap();
  let db = Arc::new(DbRepo::new(pool));

  println!("Poker Web Server Starting...");
  web::serve(db).await;
}
