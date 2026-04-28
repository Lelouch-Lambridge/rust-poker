# poker-project-cancer-the-crab

Commands:

Run Server
Prereqs: Have a Postgres database running on localhost port: 5432, with the database named: poker and user: postgres
```sh
cargo run -p Server
```

Run Client

```sh
cargo run -p Client
```

Listen to Server comms

```sh
nc -l -u 127.0.0.1 9999
```

List comms on port

```sh
lsof -i :9999
```

Send Debug commands to server

```sh
echo "DEBUG_*" | nc 127.0.0.1 9999
```
