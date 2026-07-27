//! Generic SPL Token account layout — DEX-agnostic. Every `dex-*` crate's
//! vaults are plain SPL Token accounts (Raydium, Orca, Meteora all use them),
//! so this lives in `core`, not in any single DEX crate. Layout verified
//! 2026-07-27 (stable since program deploy — every Solana wallet relies on
//! it): mint 0..32, owner 32..64, amount 64..72 (u64 LE). Total account
//! length is exactly 165 bytes.

pub const TOKEN_ACCOUNT_LEN: usize = 165;

pub fn token_account_amount(data: &[u8]) -> Option<u64> {
    if data.len() != TOKEN_ACCOUNT_LEN {
        return None;
    }
    let amount_bytes: [u8; 8] = data[64..72].try_into().ok()?;
    Some(u64::from_le_bytes(amount_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_amount_from_a_well_formed_account() {
        let mut data = vec![0u8; TOKEN_ACCOUNT_LEN];
        data[64..72].copy_from_slice(&123_456_789u64.to_le_bytes());
        assert_eq!(token_account_amount(&data), Some(123_456_789));
    }

    #[test]
    fn rejects_wrong_length() {
        assert_eq!(token_account_amount(&[0u8; 10]), None);
    }
}
