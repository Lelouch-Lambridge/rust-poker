#[allow(dead_code)]
mod clienthandler;
#[allow(dead_code)]
mod servermanager;
mod web;

use bb8::Pool;
use bb8_postgres::PostgresConnectionManager;
use tokio_postgres::NoTls;
use std::{env, net::SocketAddr, sync::Arc};

use server::db::{GameDatabase, DbRepo};

#[tokio::main]
async fn main() {
  dotenvy::dotenv().ok();
  env_logger::init();

  let database_url = env::var("DATABASE_URL")
    .unwrap_or_else(|_| database_url_from_parts());
  let bind_addr = bind_addr();

  let manager = PostgresConnectionManager::new_from_stringlike(
    database_url,
    NoTls,
  ).expect("DATABASE_URL or DB_* settings must be a valid Postgres connection config");

  let pool = Pool::builder().build(manager).await.unwrap();
  let db = Arc::new(DbRepo::new(pool));
  db.init_schema().await;

  println!("Poker Web Server Starting...");
  web::serve(db, bind_addr).await;
}

fn database_url_from_parts() -> String {
  let mut parts = vec![
    format!("host={}", env::var("DB_HOST").unwrap_or_else(|_| "localhost".to_string())),
    format!("user={}", env::var("DB_USER").unwrap_or_else(|_| "postgres".to_string())),
    format!("dbname={}", env::var("DB_NAME").unwrap_or_else(|_| "poker".to_string())),
  ];

  if let Ok(port) = env::var("DB_PORT") {
    parts.push(format!("port={}", port));
  }

  if let Ok(password) = env::var("DB_PASSWORD") {
    parts.push(format!("password={}", password));
  }

  parts.join(" ")
}

fn bind_addr() -> SocketAddr {
  env::var("BIND_ADDR")
    .or_else(|_| env::var("SERVER_BIND_ADDR"))
    .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
    .parse()
    .expect("BIND_ADDR must be a valid socket address, for example 0.0.0.0:8080")
}
