//! Append-only recorder: every raw account update is written as a
//! bincode-encoded, length-prefixed record, so Phase 4's replay engine can
//! run the exact same scanner/research code against a historical window
//! without touching mainnet. Reuses `serde`/`bincode` — no bespoke format,
//! per IMPLEMENTATION_PLAN.md.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawUpdate {
    pub pool_id: [u8; 32],
    pub slot: u64,
    pub data: Vec<u8>,
}

/// Appends one length-prefixed, bincode-encoded record to `writer`.
pub fn append(writer: &mut impl Write, update: &RawUpdate) -> io::Result<()> {
    let encoded = bincode::serialize(update).map_err(io::Error::other)?;
    writer.write_all(&(encoded.len() as u32).to_le_bytes())?;
    writer.write_all(&encoded)?;
    Ok(())
}

/// Reads one record from `reader`, or `None` at EOF — clean (between
/// records) or partial (mid-record). A partial trailing record is the
/// expected result of killing the recording process abruptly (crash, OOM,
/// `SIGKILL`) rather than shutting it down gracefully — this is an
/// append-only log, so that's normal wear, not corruption to error out on.
pub fn read_one(reader: &mut impl Read) -> io::Result<Option<RawUpdate>> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    match reader.read_exact(&mut buf) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let update = bincode::deserialize(&buf).map_err(io::Error::other)?;
    Ok(Some(update))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trips_multiple_records() {
        let updates = vec![
            RawUpdate {
                pool_id: [1u8; 32],
                slot: 10,
                data: vec![1, 2, 3],
            },
            RawUpdate {
                pool_id: [2u8; 32],
                slot: 11,
                data: vec![],
            },
        ];

        let mut buf = Vec::new();
        for u in &updates {
            append(&mut buf, u).unwrap();
        }

        let mut cursor = Cursor::new(buf);
        let mut read_back = Vec::new();
        while let Some(u) = read_one(&mut cursor).unwrap() {
            read_back.push(u);
        }
        assert_eq!(read_back, updates);
    }

    #[test]
    fn empty_input_returns_none() {
        let mut cursor = Cursor::new(Vec::new());
        assert_eq!(read_one(&mut cursor).unwrap(), None);
    }

    #[test]
    fn truncated_trailing_record_returns_none_not_error() {
        // Simulates killing the recording process mid-write: a complete
        // record followed by a length prefix whose payload never fully
        // landed on disk.
        let mut buf = Vec::new();
        append(
            &mut buf,
            &RawUpdate {
                pool_id: [1u8; 32],
                slot: 10,
                data: vec![1, 2, 3],
            },
        )
        .unwrap();
        buf.extend_from_slice(&100u32.to_le_bytes()); // claims 100 bytes follow
        buf.extend_from_slice(&[9u8; 5]); // only 5 actually written

        let mut cursor = Cursor::new(buf);
        assert!(read_one(&mut cursor).unwrap().is_some()); // the complete record
        assert_eq!(read_one(&mut cursor).unwrap(), None); // truncated one, not an error
    }
}
