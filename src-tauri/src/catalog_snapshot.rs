//! Binary gzip snapshot encoding and decoding for the local AniList catalog.
//!
//! Rather than requiring a new install to crawl AniList's public GraphQL API
//! (~22,000 titles partitioned at ~28.6 requests/minute, taking >15 minutes),
//! a complete catalog snapshot is encoded into a single gzip-compressed JSON Lines
//! payload that can be fetched in one request and imported directly into SQLite.
//! Incremental sync passes then only need to fetch updates since the snapshot.

use crate::anilist::CatalogAnime;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::{BufRead, BufReader, Read, Write};

/// Current snapshot format version. Incremented if header or serialization changes.
#[allow(dead_code)] // Supervisor wires this in the next task.
pub const FORMAT_VERSION: u32 = 1;

/// Maximum allowable decompressed snapshot payload size (256 MB) to prevent gzip bombs.
#[allow(dead_code)] // Supervisor wires this in the next task.
pub const MAX_DECOMPRESSED_BYTES: u64 = 256 * 1024 * 1024;

/// Metadata header stored as the first line of every snapshot payload.
#[allow(dead_code)] // Supervisor wires this in the next task.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SnapshotHeader {
    pub format: u32,
    pub generated_at: i64,
    pub count: usize,
}

/// Encodes catalog anime items into a gzip-compressed JSON Lines payload.
///
/// The first line contains the [`SnapshotHeader`], followed by one JSON object
/// per [`CatalogAnime`] item separated by newlines.
#[allow(dead_code)] // Supervisor wires this in the next task.
pub fn encode(generated_at: i64, items: &[CatalogAnime]) -> anyhow::Result<Vec<u8>> {
    let header = SnapshotHeader {
        format: FORMAT_VERSION,
        generated_at,
        count: items.len(),
    };
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    serde_json::to_writer(&mut encoder, &header)?;
    encoder.write_all(b"\n")?;
    for item in items {
        serde_json::to_writer(&mut encoder, item)?;
        encoder.write_all(b"\n")?;
    }
    let bytes = encoder.finish()?;
    Ok(bytes)
}

/// Decodes a gzip-compressed snapshot using the standard [`MAX_DECOMPRESSED_BYTES`] limit.
#[allow(dead_code)] // Supervisor wires this in the next task.
pub fn decode(bytes: &[u8]) -> anyhow::Result<(SnapshotHeader, Vec<CatalogAnime>)> {
    decode_with_limit(bytes, MAX_DECOMPRESSED_BYTES)
}

/// Internal decoder with an explicit decompression byte limit for safety and testability.
fn decode_with_limit(
    bytes: &[u8],
    max_decompressed_bytes: u64,
) -> anyhow::Result<(SnapshotHeader, Vec<CatalogAnime>)> {
    if bytes.is_empty() {
        anyhow::bail!("empty snapshot input");
    }

    let decoder = GzDecoder::new(bytes);
    let limited = decoder.take(max_decompressed_bytes + 1);
    let mut reader = BufReader::new(limited);

    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => {
            if reader.get_ref().limit() == 0 {
                anyhow::bail!(
                    "decompressed snapshot exceeded maximum limit of {max_decompressed_bytes} bytes"
                );
            }
            anyhow::bail!("unexpected end of file reading snapshot header");
        }
        Ok(_) => {
            if reader.get_ref().limit() == 0 {
                anyhow::bail!(
                    "decompressed snapshot exceeded maximum limit of {max_decompressed_bytes} bytes"
                );
            }
        }
        Err(e) => {
            if reader.get_ref().limit() == 0 {
                anyhow::bail!(
                    "decompressed snapshot exceeded maximum limit of {max_decompressed_bytes} bytes"
                );
            }
            return Err(anyhow::anyhow!("failed to decompress snapshot header: {e}"));
        }
    }

    let header: SnapshotHeader = serde_json::from_str(&line)
        .map_err(|e| anyhow::anyhow!("failed to parse snapshot header: {e}"))?;

    if header.format != FORMAT_VERSION {
        anyhow::bail!(
            "unsupported snapshot format version {}, expected {}",
            header.format,
            FORMAT_VERSION
        );
    }

    let mut items = Vec::with_capacity(header.count);
    let mut line_num = 1;

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => {
                if reader.get_ref().limit() == 0 {
                    anyhow::bail!(
                        "decompressed snapshot exceeded maximum limit of {max_decompressed_bytes} bytes"
                    );
                }
                break;
            }
            Ok(_) => {
                line_num += 1;
                if reader.get_ref().limit() == 0 {
                    anyhow::bail!(
                        "decompressed snapshot exceeded maximum limit of {max_decompressed_bytes} bytes"
                    );
                }
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let item: CatalogAnime = serde_json::from_str(trimmed).map_err(|e| {
                    anyhow::anyhow!("failed to parse CatalogAnime on line {line_num}: {e}")
                })?;
                items.push(item);
            }
            Err(e) => {
                if reader.get_ref().limit() == 0 {
                    anyhow::bail!(
                        "decompressed snapshot exceeded maximum limit of {max_decompressed_bytes} bytes"
                    );
                }
                return Err(anyhow::anyhow!(
                    "failed to decompress snapshot on line {}: {e}",
                    line_num + 1
                ));
            }
        }
    }

    if items.len() != header.count {
        anyhow::bail!(
            "snapshot item count mismatch: header specified {}, but found {}",
            header.count,
            items.len()
        );
    }

    Ok((header, items))
}

/// Exports the local database catalog as a gzip-compressed snapshot payload.
#[allow(dead_code)] // Supervisor wires this in the next task.
pub fn export_to_bytes(db: &crate::db::Db, generated_at: i64) -> anyhow::Result<Vec<u8>> {
    encode(generated_at, &db.catalog_all_for_snapshot()?)
}

/// Decodes and imports a snapshot payload into the local database, returning header and count inserted.
#[allow(dead_code)] // Supervisor wires this in the next task.
pub fn import_from_bytes(
    db: &crate::db::Db,
    bytes: &[u8],
) -> anyhow::Result<(SnapshotHeader, usize)> {
    let (header, items) = decode(bytes)?;
    let inserted = db.import_catalog_snapshot(&items)?;
    Ok((header, inserted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anilist::{CatalogRelation, CatalogTag};
    use crate::db::Db;

    fn sample_anime(id: i64, title: &str) -> CatalogAnime {
        CatalogAnime {
            id,
            title: title.into(),
            title_romaji: Some(format!("{title} Romaji")),
            title_english: Some(format!("{title} English")),
            synonyms: vec![format!("{title} Syn 1"), format!("{title} Syn 2")],
            cover_url: Some(format!("https://example.com/{id}.jpg")),
            banner_url: Some(format!("https://example.com/{id}_banner.jpg")),
            format: Some("TV".into()),
            genres: vec!["Action".into(), "Sci-Fi".into()],
            episodes: Some(12),
            average_score: Some(85),
            popularity: Some(1000 * id),
            url: format!("https://anilist.co/anime/{id}"),
            status: Some("FINISHED".into()),
            duration: Some(24),
            studio: Some("Bones".into()),
            start_date: Some(1600000000 + id),
            next_airing_at: Some(1700000000 + id),
            next_episode: Some(2),
            tags: vec![
                CatalogTag {
                    name: "Cyberpunk".into(),
                    rank: 60,
                },
                CatalogTag {
                    name: "Time Travel".into(),
                    rank: 80,
                },
            ],
            relations: vec![
                CatalogRelation {
                    related_id: id + 100,
                    relation_type: "PREQUEL".into(),
                },
                CatalogRelation {
                    related_id: id + 200,
                    relation_type: "SEQUEL".into(),
                },
            ],
        }
    }

    #[test]
    fn test_encode_decode_roundtrip_full_fidelity() {
        let items = vec![
            sample_anime(1, "First"),
            sample_anime(2, "Second"),
            sample_anime(3, "Third"),
        ];
        let bytes = encode(1_700_000_000, &items).unwrap();
        let (header, decoded_items) = decode(&bytes).unwrap();

        assert_eq!(header.format, FORMAT_VERSION);
        assert_eq!(header.generated_at, 1_700_000_000);
        assert_eq!(header.count, 3);
        assert_eq!(decoded_items, items);
    }

    #[test]
    fn test_db_roundtrip_export_and_import() {
        let a = Db::open(":memory:").unwrap();
        let b = Db::open(":memory:").unwrap();

        let anime1 = sample_anime(1, "Anime One");
        let anime2 = sample_anime(2, "Anime Two");
        let anime3 = sample_anime(3, "Anime Three");

        let batch = [(&anime1, 0), (&anime2, 1), (&anime3, 2)];
        a.upsert_catalog_anime_batch(&batch).unwrap();

        let bytes = export_to_bytes(&a, 1_700_000_000).unwrap();
        let (header, inserted) = import_from_bytes(&b, &bytes).unwrap();

        assert_eq!(header.format, FORMAT_VERSION);
        assert_eq!(header.generated_at, 1_700_000_000);
        assert_eq!(header.count, 3);
        assert_eq!(inserted, 3);
        assert_eq!(b.catalog_count().unwrap(), 3);

        let items_a = a.catalog_all_for_snapshot().unwrap();
        let items_b = b.catalog_all_for_snapshot().unwrap();
        assert_eq!(items_a.len(), 3);
        assert_eq!(items_b, items_a);
    }

    #[test]
    fn test_decode_rejects_empty_input() {
        let err = decode(b"").unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn test_decode_rejects_random_bytes() {
        let err = decode(b"this is definitely not a gzip stream").unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn test_decode_rejects_unsupported_format_version() {
        let header = SnapshotHeader {
            format: 2,
            generated_at: 1_700_000_000,
            count: 0,
        };
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        serde_json::to_writer(&mut encoder, &header).unwrap();
        encoder.write_all(b"\n").unwrap();
        let bytes = encoder.finish().unwrap();

        let err = decode(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains('2'),
            "error message must name the unsupported version: {msg}"
        );
    }

    #[test]
    fn test_decode_rejects_count_mismatch() {
        let header = SnapshotHeader {
            format: FORMAT_VERSION,
            generated_at: 1_700_000_000,
            count: 5,
        };
        let items = vec![
            sample_anime(1, "A"),
            sample_anime(2, "B"),
            sample_anime(3, "C"),
        ];
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        serde_json::to_writer(&mut encoder, &header).unwrap();
        encoder.write_all(b"\n").unwrap();
        for item in &items {
            serde_json::to_writer(&mut encoder, item).unwrap();
            encoder.write_all(b"\n").unwrap();
        }
        let bytes = encoder.finish().unwrap();

        let err = decode(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("mismatch") || msg.contains('5'),
            "error should mention count mismatch: {msg}"
        );
    }

    #[test]
    fn test_decode_rejects_malformed_body_line_with_line_number() {
        let header = SnapshotHeader {
            format: FORMAT_VERSION,
            generated_at: 1_700_000_000,
            count: 2,
        };
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        serde_json::to_writer(&mut encoder, &header).unwrap();
        encoder.write_all(b"\n").unwrap();
        // Line 2: valid item
        let item1 = sample_anime(1, "Item 1");
        serde_json::to_writer(&mut encoder, &item1).unwrap();
        encoder.write_all(b"\n").unwrap();
        // Line 3: invalid JSON for 2nd item
        encoder.write_all(b"not json\n").unwrap();
        let bytes = encoder.finish().unwrap();

        let err = decode(&bytes).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("line 3"),
            "error message must contain 'line 3': {msg}"
        );
    }

    #[test]
    fn test_decode_rejects_exceeding_byte_limit_without_full_allocation() {
        let items: Vec<CatalogAnime> = (1..=10)
            .map(|i| sample_anime(i, &format!("Anime Number {i} with a longer title")))
            .collect();
        let bytes = encode(1_700_000_000, &items).unwrap();

        let err = decode_with_limit(&bytes, 1024).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("limit") || msg.contains("1024"),
            "error should indicate size limit exceeded: {msg}"
        );
    }

    #[test]
    fn test_decode_never_panics_on_malformed_input() {
        let inputs: Vec<&[u8]> = vec![
            b"",
            b"\x1f",
            b"\x1f\x8b",
            b"\x1f\x8b\x08",
            b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\x03\x03\x00\x00\x00\x00\x00\x00\x00\x00\x00",
            b"not gzip at all\0\xff\xfe",
            &[0x1f, 0x8b, 0x08, 0x00, 0xff, 0xff, 0xff],
        ];
        for input in inputs {
            let res = std::panic::catch_unwind(|| {
                let _ = decode(input);
            });
            assert!(res.is_ok(), "decode must not panic on malformed input");
        }
    }
}
