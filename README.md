# queries-rusqlite

[`queries`](https://docs.rs/queries/0.2.0/queries/)-style query declarations for
[`rusqlite`](https://docs.rs/rusqlite/), with synchronous methods.

Requires Rust 1.95.0 or later.

Install from crates.io:

```toml
[dependencies]
queries-rusqlite = "0.2"
rusqlite = { version = "0.40.2", default-features = false }
```

## Queries

```rust
use queries_rusqlite::{FromRow, queries};

#[derive(FromRow)]
struct User {
    id: i64,
    name: String,
}

#[queries]
trait Users {
    #[query = "INSERT INTO users (id, name) VALUES (?1, ?2)"]
    fn insert(id: i64, name: &str);

    #[query = "SELECT id, name FROM users WHERE id = ?1"]
    fn get(id: i64) -> Option<User>;
}

fn main() -> rusqlite::Result<()> {
    let connection = rusqlite::Connection::open_in_memory()?;
    connection.execute_batch("CREATE TABLE users (id INTEGER, name TEXT)")?;

    let users = Users::from_conn(&connection);
    users.insert(1, "Ada")?;
    assert_eq!(users.get(1)?.unwrap().name, "Ada");
    Ok(())
}
```

See the API documentation with `cargo doc --open`.

## License

BSD-3-Clause. The return type dispatch is adapted from
[`queries`](https://github.com/alex/queries-rs); its copyright notice and
license are retained in `LICENSE`.
