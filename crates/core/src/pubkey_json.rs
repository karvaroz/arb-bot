//! `solana_sdk::Pubkey`'s own `Serialize` impl writes a raw `[u8; 32]` even
//! for JSON (confirmed empirically: `/pools` returned a byte array, not a
//! base58 string) — fine for `bincode` (the recorder/replay log), useless
//! for a REST API a human or dashboard reads. Use `#[serde(with =
//! "arb_core::pubkey_json")]` (or `::vec` for `Vec<Pubkey>`) only on fields
//! that cross that JSON boundary — leave `PoolMetadata`/`RawUpdate` alone,
//! they still go through `bincode` in `recorder`/`replay`.

use serde::{Deserialize, Deserializer, Serializer};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;

pub fn serialize<S: Serializer>(pubkey: &Pubkey, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&pubkey.to_string())
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Pubkey, D::Error> {
    let s = String::deserialize(d)?;
    Pubkey::from_str(&s).map_err(serde::de::Error::custom)
}

pub mod vec {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use solana_sdk::pubkey::Pubkey;
    use std::str::FromStr;

    pub fn serialize<S: Serializer>(pubkeys: &[Pubkey], s: S) -> Result<S::Ok, S::Error> {
        pubkeys
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Pubkey>, D::Error> {
        Vec::<String>::deserialize(d)?
            .into_iter()
            .map(|s| Pubkey::from_str(&s).map_err(serde::de::Error::custom))
            .collect()
    }
}
