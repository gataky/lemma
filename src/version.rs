// The lemma generator version, sourced from Cargo.toml at compile time. Stored
// as `lemma_version` in the database's meta table.

pub const LEMMA_VERSION: &str = env!("CARGO_PKG_VERSION");
