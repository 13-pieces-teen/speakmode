use super::*;
use std::io::Write;

struct Fixture { path: PathBuf, entries: usize }
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("speakmode-typeless-cache-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        let mut index = vec![0; INDEX_HEADER + 16 * 4];
        put(&mut index, 0, 0xc103_cac3); put(&mut index, 4, 0x30000); put(&mut index, 28, 16);
        std::fs::write(path.join("index"), index).unwrap();
        for (number, size) in [(0, 36), (1, 256)] {
            let mut block = vec![0; BLOCK_HEADER as usize];
            put(&mut block, 0, 0xc104_cac3); put(&mut block, 4, 0x20000); put(&mut block, 8, number);
            put(&mut block, 12, size); put(&mut block, 20, 16);
            std::fs::write(path.join(format!("data_{number}")), block).unwrap();
        }
        Self { path, entries: 0 }
    }
    fn add(&mut self, key: &str, time: i64, encoding: &str, body: &[u8]) {
        let number = self.entries;
        self.entries += 1;
        let address = 0xa001_0000 | number as u32;
        let info = metadata(time, encoding);
        let mut index = std::fs::read(self.path.join("index")).unwrap();
        // Identical full-view requests collide and exercise linked entries.
        let hash = persistent_hash(key.as_bytes());
        let bucket_offset = INDEX_HEADER + (hash as usize & 15) * 4;
        let next = u32_at(&index, bucket_offset).unwrap();
        put(&mut index, bucket_offset, address); put(&mut index, 8, self.entries as u32);
        std::fs::write(self.path.join("index"), index).unwrap();
        let mut entry = vec![0; 256];
        put(&mut entry, 0, hash);
        put(&mut entry, 4, next); put(&mut entry, 8, 0x9000_0000 | number as u32);
        put(&mut entry, 32, key.len() as u32); entry[96..96+key.len()].copy_from_slice(key.as_bytes());
        put(&mut entry, 40, info.len() as u32); put(&mut entry, 44, body.len() as u32);
        put(&mut entry, 56, 0x8000_0000 | (number as u32 * 2 + 1));
        put(&mut entry, 60, 0x8000_0000 | (number as u32 * 2 + 2));
        rehash(&mut entry, 0, 92);
        let mut rank = vec![0; 36]; put(&mut rank, 24, address); rehash(&mut rank, 0, 32);
        for (file, size, bytes) in [(0, 36, rank), (1, 256, entry)] {
            let name = self.path.join(format!("data_{file}"));
            let mut block = std::fs::read(&name).unwrap();
            block[80+number/8] |= 1 << (number%8);
            block.resize(BLOCK_HEADER as usize + (number + 1) * size, 0);
            block[BLOCK_HEADER as usize + number*size..][..size].copy_from_slice(&bytes);
            std::fs::write(name, block).unwrap();
        }
        std::fs::write(self.path.join(format!("f_{:06x}", number*2+1)), info).unwrap();
        std::fs::write(self.path.join(format!("f_{:06x}", number*2+2)), body).unwrap();
    }
    fn mutate(&self, name: &str, action: impl FnOnce(&mut Vec<u8>)) {
        let path = self.path.join(name); let mut bytes = std::fs::read(&path).unwrap(); action(&mut bytes); std::fs::write(path, bytes).unwrap();
    }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.path); } }
fn put(bytes: &mut [u8], offset: usize, value: u32) { bytes[offset..offset+4].copy_from_slice(&value.to_le_bytes()); }
fn rehash(bytes: &mut [u8], start: usize, offset: usize) { let hash=persistent_hash(&bytes[start..start+offset]);put(bytes,start+offset,hash); }
fn metadata(time: i64, encoding: &str) -> Vec<u8> {
    let headers = format!("HTTP/1.1 200\0content-type:application/json\0content-encoding:{encoding}\0\0");
    let mut bytes = vec![0; 28]; put(&mut bytes, 4, 3);
    bytes[16..24].copy_from_slice(&time.to_le_bytes()); put(&mut bytes, 24, headers.len() as u32);
    bytes.extend_from_slice(headers.as_bytes()); while bytes.len()%4 != 0 { bytes.push(0); }
    let length = bytes.len() as u32 - 4; put(&mut bytes, 0, length); bytes
}
fn payload(terms: &[&str], total: usize) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"status":"OK","data":{"words":terms.iter().map(|term|serde_json::json!({"term":term,"user_dictionary_id":"synthetic","replace_targets":[]})).collect::<Vec<_>>(),"total_count":total}})).unwrap()
}
fn key() -> String { format!("1/0/{ENDPOINT}?size=150") }
fn compressed(bytes: &[u8]) -> Vec<u8> {
    let mut result = Vec::new(); { let mut writer = brotli::CompressorWriter::new(&mut result, 4096, 3, 20); writer.write_all(bytes).unwrap(); } result
}

#[test]
fn reads_complete_active_brotli_dictionary_without_source_writes() {
    let mut fixture = Fixture::new();
    fixture.add(&key(), 13_436_004_512_339_405, "br", &compressed(&payload(&["SpeakMode", "清华大学"], 2)));
    let before: Vec<_> = std::fs::read_dir(&fixture.path).unwrap().map(|entry| {let path=entry.unwrap().path();let bytes=std::fs::read(&path).unwrap();(path,bytes)}).collect();
    let result = read_cache(&fixture.path).unwrap();
    assert_eq!(result.terms, ["SpeakMode", "清华大学"]); assert!(result.cached_at.unwrap().starts_with("2026-10-09"));
    for (path, bytes) in before { assert_eq!(std::fs::read(path).unwrap(), bytes); }
}

#[test]
fn ignores_filtered_newer_responses_and_unindexed_deleted_entries() {
    let mut fixture = Fixture::new();
    fixture.add(&key(), 10, "identity", &payload(&["kept"], 1));
    fixture.add(&(key()+"&auto=true"), 30, "unsupported", b"private unrelated data");
    fixture.add(&key(), 50, "identity", &payload(&["deleted"], 1));
    fixture.mutate("index", |bytes| put(bytes, INDEX_HEADER + (persistent_hash(key().as_bytes()) as usize & 15) * 4, 0xa001_0000));
    assert_eq!(read_cache(&fixture.path).unwrap().terms, ["kept"]);
}

#[test]
fn latest_response_replaces_old_words_instead_of_merging_cache_history() {
    let mut fixture = Fixture::new();
    fixture.add(&key(), 10, "identity", &payload(&["old"], 1));
    fixture.add(&key(), 20, "identity", &payload(&["new"], 1));
    assert_eq!(read_cache(&fixture.path).unwrap().terms, ["new"]);
}

#[test]
fn incomplete_latest_response_does_not_fall_back_to_older_complete_cache() {
    let mut fixture = Fixture::new(); fixture.add(&key(), 10, "identity", &payload(&["old"], 1));
    fixture.add(&key(), 20, "identity", &payload(&["new"], 2));
    assert_eq!(read_cache(&fixture.path).err(), Some(TypelessImportError::Incomplete));
}

#[test]
fn unsupported_latest_encoding_does_not_fall_back_to_older_cache() {
    let mut fixture = Fixture::new(); fixture.add(&key(), 10, "identity", &payload(&["old"], 1));
    fixture.add(&key(), 20, "zstd", &payload(&["new"], 1));
    assert_eq!(read_cache(&fixture.path).err(), Some(TypelessImportError::Unsupported));
}

#[test]
fn rejects_unallocated_or_dirty_dictionary_blocks() {
    let mut fixture = Fixture::new(); fixture.add(&key(), 10, "identity", &payload(&["word"], 1));
    fixture.mutate("data_0", |bytes| { put(bytes, BLOCK_HEADER as usize + 28, 42); rehash(bytes,BLOCK_HEADER as usize,32); });
    assert_eq!(read_cache(&fixture.path).err(), Some(TypelessImportError::Changed));
    fixture.mutate("data_0", |bytes| { put(bytes, BLOCK_HEADER as usize + 28, 0); rehash(bytes,BLOCK_HEADER as usize,32); });
    fixture.mutate("data_1", |bytes| bytes[80] = 0);
    assert_eq!(read_cache(&fixture.path).err(), Some(TypelessImportError::Changed));
}

#[test]
fn rank_cannot_point_to_another_active_dictionary_entry() {
    let mut fixture = Fixture::new(); fixture.add(&key(), 10, "identity", &payload(&["older"], 1));
    fixture.add(&key(), 20, "identity", &payload(&["newer"], 1));
    // The older entry is visited after the newer one; its ranking node must not
    // be accepted merely because it points to an address in the visited set.
    fixture.mutate("data_0", |bytes| { put(bytes, BLOCK_HEADER as usize + 24, 0xa001_0001); rehash(bytes,BLOCK_HEADER as usize,32); });
    assert_eq!(read_cache(&fixture.path).err(), Some(TypelessImportError::Unsupported));
}

#[test]
fn cyclic_index_or_wrong_bucket_is_rejected() {
    let mut fixture = Fixture::new(); fixture.add(&key(), 10, "identity", &payload(&["word"], 1));
    fixture.mutate("data_1", |bytes| { put(bytes, BLOCK_HEADER as usize + 4, 0xa001_0000); rehash(bytes,BLOCK_HEADER as usize,92); });
    assert_eq!(read_cache(&fixture.path).err(), Some(TypelessImportError::Unsupported));
    fixture.mutate("data_1", |bytes| { put(bytes, BLOCK_HEADER as usize + 4, 0); put(bytes, BLOCK_HEADER as usize, 1); rehash(bytes,BLOCK_HEADER as usize,92); });
    assert_eq!(read_cache(&fixture.path).err(), Some(TypelessImportError::Unsupported));
}

#[test]
fn changed_snapshot_is_detected_without_any_import() {
    let fixture = Fixture::new(); let mut reader = CacheReader::new(&fixture.path);
    reader.span("index", 0, INDEX_HEADER).unwrap(); fixture.mutate("index", |bytes| put(bytes, 20, 42));
    assert_eq!(reader.verify(), Err(TypelessImportError::Changed));
}

#[test]
fn rejects_unknown_cache_format_and_truncated_external_body() {
    let mut fixture = Fixture::new(); fixture.add(&key(), 10, "identity", &payload(&["word"], 1));
    fixture.mutate("f_000002", |bytes| bytes.truncate(3));
    assert_eq!(read_cache(&fixture.path).err(), Some(TypelessImportError::Unsupported));
    fixture.mutate("index", |bytes| put(bytes, 4, 0x40000));
    assert_eq!(read_cache(&fixture.path).err(), Some(TypelessImportError::Unsupported));
}

#[test]
fn rejects_brotli_expansion_beyond_limit() {
    let mut fixture = Fixture::new(); fixture.add(&key(), 10, "br", &compressed(&vec![b' ';MAX_DECODED+1]));
    assert_eq!(read_cache(&fixture.path).err(), Some(TypelessImportError::TooLarge));
}

#[test]
fn response_schema_must_be_complete_and_terms_cannot_inject_new_rows() {
    assert_eq!(response_terms(&payload(&[], 0)).err(), Some(TypelessImportError::Empty));
    assert_eq!(response_terms(&payload(&["one\ntwo"], 1)).err(), Some(TypelessImportError::Unsupported));
    assert_eq!(response_terms(br#"{"status":"ERROR","data":{"words":[],"total_count":0}}"#).err(), Some(TypelessImportError::Incomplete));
    assert_eq!(response_terms(br#"{"status":"OK","data":{"words":[{}],"total_count":1}}"#).err(), Some(TypelessImportError::Unsupported));
}

#[test]
fn accepts_only_exact_host_endpoint_and_unfiltered_query() {
    assert!(is_full_dictionary_key(&key()));
    for other in [key()+"&search=abc",key()+"&auto=false",key()+"&page=2",key()+"#test",key().replace("api.typeless.com","api.typeless.com.evil"),key().replace("https://","http://"),key().replace("size=150","size=0")] {
        assert!(!is_full_dictionary_key(&other));
    }
}

#[test]
fn truncated_or_independently_compressed_http_response_is_rejected() {
    let mut bytes = metadata(10,"br"); put(&mut bytes,4,3|(1<<12));
    assert_eq!(response_metadata(&bytes).err(),Some(TypelessImportError::Incomplete));
    bytes=metadata(10,"br");put(&mut bytes,4,3|(1<<31));put(&mut bytes,8,1<<4);
    assert_eq!(response_metadata(&bytes).err(),Some(TypelessImportError::Unsupported));
}

#[test]
fn stable_corrupted_body_pointer_cannot_import_an_older_valid_response() {
    let mut fixture = Fixture::new();
    fixture.add(&key(), 10, "identity", &payload(&["old"], 1));
    fixture.add(&key(), 20, "identity", &payload(&["new"], 1));
    // Same-size, otherwise valid JSON. Changing only the new entry's body
    // pointer is stable across reads but invalidates its persisted metadata hash.
    fixture.mutate("data_1", |bytes| put(bytes, BLOCK_HEADER as usize+256+60, 0x8000_0002));
    assert_eq!(read_cache(&fixture.path).err(),Some(TypelessImportError::Unsupported));
}

#[test]
fn corrupted_rank_and_full_key_hash_are_rejected_even_with_valid_bucket() {
    let mut fixture = Fixture::new(); fixture.add(&key(), 10, "identity", &payload(&["word"], 1));
    fixture.mutate("data_0", |bytes| bytes[BLOCK_HEADER as usize] ^= 1);
    assert_eq!(read_cache(&fixture.path).err(),Some(TypelessImportError::Unsupported));
    fixture.mutate("data_0", |bytes| bytes[BLOCK_HEADER as usize] ^= 1);
    fixture.mutate("data_1", |bytes| {
        // Change a high key-hash bit, retaining the low hash-table bucket bits;
        // then correctly update self_hash to isolate the full-key validation.
        let start=BLOCK_HEADER as usize;let hash=u32_at(bytes,start).unwrap()^0x1000;
        put(bytes,start,hash);rehash(bytes,start,92);
    });
    assert_eq!(read_cache(&fixture.path).err(),Some(TypelessImportError::Unsupported));
}

#[test]
fn zero_self_hash_is_accepted_for_chromium_legacy_compatibility() {
    let mut fixture = Fixture::new(); fixture.add(&key(), 10, "identity", &payload(&["word"], 1));
    fixture.mutate("data_1",|bytes|put(bytes,BLOCK_HEADER as usize+92,0));
    fixture.mutate("data_0",|bytes|put(bytes,BLOCK_HEADER as usize+32,0));
    assert_eq!(read_cache(&fixture.path).unwrap().terms,["word"]);
}

#[test]
fn persistent_hash_matches_observed_public_cache_key_and_signed_tail_vectors() {
    assert_eq!(persistent_hash(key().as_bytes()),0xb830_22d6);
    assert_eq!(persistent_hash(b""),0);
    // Generated independently from Chromium's published C implementation.
    assert_eq!(persistent_hash(&[0xff]),0);
    assert_eq!(persistent_hash(&[0x80]),0xf305_33c4);
    assert_eq!(persistent_hash(&[1,2,0xff]),0x994a_9cb8);
    assert_eq!(persistent_hash(b"hello"),0xb09d_c87b);
    assert_eq!(persistent_hash(&(0..=255).collect::<Vec<u8>>()),0xe4ee_f917);
}
