//! Explicit, local-only import of the last complete Typeless dictionary response.
//! This does not open its account store, transcript database, recordings or APIs.
//! Format reference: chromium/chromium net/disk_cache/blockfile/{disk_format,
//! disk_format_base,addr}.h and net/http/http_response_info.cc. Verified against
//! Typeless Windows 2.8.1. Unsupported or incomplete caches fail without importing.

use std::{collections::{HashMap, HashSet}, fs::{File, OpenOptions}, io::{Read, Seek, SeekFrom}, path::{Path, PathBuf}};
use serde::Deserialize;

const INDEX_HEADER: usize = 368;
const BLOCK_HEADER: u64 = 8192;
const MAX_INDEX: usize = INDEX_HEADER + 4 * 1_048_576;
const MAX_ENTRIES: usize = 16_384;
const MAX_BODY: usize = 1024 * 1024;
const MAX_DECODED: usize = 4 * MAX_BODY;
const MAX_OBSERVED: usize = 16 * MAX_BODY;
const MAX_FILE: u64 = 272 * 1024 * 1024;
const ENDPOINT: &str = "https://api.typeless.com/user/dictionary/list";

/// Do not derive Debug: imported terms must never leak into diagnostic logs.
pub struct ImportedTerms {
    pub terms: Vec<String>,
    pub cached_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TypelessImportError {
    #[error("typeless dictionary cache unavailable")]
    Unavailable,
    #[error("typeless dictionary cache unsupported")]
    Unsupported,
    #[error("typeless dictionary cache incomplete")]
    Incomplete,
    #[error("typeless dictionary cache changed")]
    Changed,
    #[error("typeless dictionary cache limit exceeded")]
    TooLarge,
    #[error("typeless dictionary cache empty")]
    Empty,
}
impl TypelessImportError {
    pub fn code(self) -> &'static str { match self {
        Self::Unavailable => "typeless_cache_unavailable",
        Self::Unsupported => "typeless_cache_unsupported",
        Self::Incomplete => "typeless_cache_incomplete",
        Self::Changed => "typeless_cache_changed",
        Self::TooLarge => "typeless_cache_too_large",
        Self::Empty => "typeless_dictionary_empty",
    }}
    pub fn message(self) -> &'static str { match self {
        Self::Unavailable => "未找到 Typeless 词典缓存，请先在 Typeless 打开词典的“所有”页面，再重试",
        Self::Unsupported => "当前 Typeless 缓存格式暂不支持，可在“新词 → 批量添加”中手动添加",
        Self::Incomplete => "Typeless 词典缓存不完整，请先打开词典的“所有”页面并加载全部词汇，再重试",
        Self::Changed => "Typeless 正在更新词典，请稍后重试",
        Self::TooLarge => "Typeless 词典缓存超过读取范围，可在“新词 → 批量添加”中手动添加",
        Self::Empty => "Typeless 中没有可导入的词汇",
    }}
}
type Result<T> = std::result::Result<T, TypelessImportError>;

/// Called only by the user's import action. The source is fixed, never IPC supplied.
pub fn read_local() -> Result<ImportedTerms> {
    #[cfg(not(any(windows, target_os="macos")))] { Err(TypelessImportError::Unavailable) }
    #[cfg(target_os="macos")] {
        let home = std::env::var_os("HOME").ok_or(TypelessImportError::Unavailable)?;
        let support = PathBuf::from(home).join("Library/Application Support");
        let roots = ["Typeless", "Typeless.exe"].map(|name| support.join(name).join("Partitions/no-proxy-session/Cache/Cache_Data"));
        let root = roots.iter().find(|path| path.join("index").is_file()).ok_or(TypelessImportError::Unavailable)?;
        read_cache(root)
    }
    #[cfg(windows)] {
        let appdata = std::env::var_os("APPDATA").ok_or(TypelessImportError::Unavailable)?;
        let root = PathBuf::from(appdata).join("Typeless.exe/Partitions/no-proxy-session/Cache/Cache_Data");
        read_cache(&root)
    }
}

fn read_cache(root: &Path) -> Result<ImportedTerms> {
    // No cache copies, account material or temporary export files are created.
    for _ in 0..2 {
        match read_snapshot(root) {
            Err(TypelessImportError::Changed) => continue,
            result => return result,
        }
    }
    Err(TypelessImportError::Changed)
}

fn open_read(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)] {
        use std::os::windows::fs::OpenOptionsExt;
        // Chromium keeps the cache open. Sharing permits reads, never writes.
        options.share_mode(1 | 2 | 4);
    }
    options.open(path).map_err(|_| TypelessImportError::Unavailable)
}

struct CacheReader<'a> {
    root: &'a Path,
    observed: HashMap<(String, u64, usize), Vec<u8>>,
    observed_bytes: usize,
}
impl<'a> CacheReader<'a> {
    fn new(root: &'a Path) -> Self { Self { root, observed: HashMap::new(), observed_bytes: 0 } }
    fn raw(&self, name: &str, offset: u64, size: usize) -> Result<Vec<u8>> {
        if size > MAX_INDEX { return Err(TypelessImportError::TooLarge); }
        let mut file = open_read(&self.root.join(name))?;
        let len = file.metadata().map_err(|_| TypelessImportError::Unavailable)?.len();
        if len > MAX_FILE || offset.checked_add(size as u64).is_none_or(|end| end > len) {
            return Err(TypelessImportError::Unsupported);
        }
        file.seek(SeekFrom::Start(offset)).map_err(|_| TypelessImportError::Unavailable)?;
        let mut bytes = vec![0; size];
        file.read_exact(&mut bytes).map_err(|_| TypelessImportError::Changed)?;
        Ok(bytes)
    }
    fn span(&mut self, name: &str, offset: u64, size: usize) -> Result<Vec<u8>> {
        let key = (name.to_owned(), offset, size);
        if let Some(bytes) = self.observed.get(&key) { return Ok(bytes.clone()); }
        self.observed_bytes = self.observed_bytes.checked_add(size).ok_or(TypelessImportError::TooLarge)?;
        if self.observed_bytes > MAX_OBSERVED { return Err(TypelessImportError::TooLarge); }
        let bytes = self.raw(name, offset, size)?;
        self.observed.insert(key, bytes.clone());
        Ok(bytes)
    }
    fn verify(&self) -> Result<()> {
        // Reopen every observed span. Index links, entry/rank metadata and response
        // bytes must all still match; in-flight writes cause a bounded retry.
        for ((name, offset, size), before) in &self.observed {
            if self.raw(name, *offset, *size)? != *before { return Err(TypelessImportError::Changed); }
        }
        Ok(())
    }
    fn stream(&mut self, address: u32, size: usize) -> Result<Vec<u8>> {
        if size == 0 || size > MAX_BODY || address & 0x8000_0000 == 0 { return Err(TypelessImportError::Unsupported); }
        let kind = (address >> 28) & 7;
        if kind == 0 {
            return self.span(&format!("f_{:06x}", address & 0x0fff_ffff), 0, size);
        }
        let block_size = match kind { 1 => 36, 2 => 256, 3 => 1024, 4 => 4096, _ => return Err(TypelessImportError::Unsupported) };
        let count = ((address >> 24) & 3) + 1;
        let number = (address >> 16) & 255;
        let start = address & 65535;
        if address & 0x0c00_0000 != 0 || size > (block_size * count) as usize { return Err(TypelessImportError::Unsupported); }
        let name = format!("data_{number}");
        let header = self.span(&name, 0, 80)?;
        if u32_at(&header, 0)? != 0xc104_cac3 || u32_at(&header, 4)? != 0x20000
            || u16_at(&header, 8)? as u32 != number || u32_at(&header, 12)? != block_size
            || start + count > u32_at(&header, 20)? || start + count > 64_896 {
            return Err(TypelessImportError::Unsupported);
        }
        if u32_at(&header, 56)? != 0 { return Err(TypelessImportError::Changed); }
        // Only currently allocated blocks are readable. Never carve stale data.
        let bitmap_start = start / 8;
        let bitmap_end = (start + count - 1) / 8;
        let bitmap = self.span(&name, 80 + bitmap_start as u64, (bitmap_end - bitmap_start + 1) as usize)?;
        for block in start..start + count {
            if bitmap[(block / 8 - bitmap_start) as usize] & (1 << (block % 8)) == 0 {
                return Err(TypelessImportError::Changed);
            }
        }
        self.span(&name, BLOCK_HEADER + start as u64 * block_size as u64, size)
    }
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16> { Ok(u16::from_le_bytes(bytes.get(offset..offset+2).ok_or(TypelessImportError::Unsupported)?.try_into().unwrap())) }
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> { Ok(u32::from_le_bytes(bytes.get(offset..offset+4).ok_or(TypelessImportError::Unsupported)?.try_into().unwrap())) }
fn i64_at(bytes: &[u8], offset: usize) -> Result<i64> { Ok(i64::from_le_bytes(bytes.get(offset..offset+8).ok_or(TypelessImportError::Unsupported)?.try_into().unwrap())) }

struct Candidate { response_time: i64, encoding: String, body_address: u32, body_size: usize }

fn read_snapshot(root: &Path) -> Result<ImportedTerms> {
    let mut reader = CacheReader::new(root);
    let index_head = reader.span("index", 0, INDEX_HEADER)?;
    if u32_at(&index_head, 0)? != 0xc103_cac3 || u32_at(&index_head, 4)? != 0x30000 || u32_at(&index_head, 56)? != 0 {
        return Err(TypelessImportError::Unsupported);
    }
    let mut table_len = u32_at(&index_head, 28)? as usize;
    if table_len == 0 { table_len = 65_536; }
    if !table_len.is_power_of_two() || table_len > 1_048_576 || u32_at(&index_head, 8)? as usize > MAX_ENTRIES {
        return Err(TypelessImportError::TooLarge);
    }
    let table = reader.span("index", INDEX_HEADER as u64, table_len * 4)?;
    let mut visited = HashSet::new();
    let mut candidates = Vec::new();
    for bucket in 0..table_len {
        let mut address = u32_at(&table, bucket * 4)?;
        while address != 0 {
            let entry_address = address;
            if !visited.insert(address) || visited.len() > MAX_ENTRIES { return Err(TypelessImportError::Unsupported); }
            if (address >> 28) & 7 != 2 { return Err(TypelessImportError::Unsupported); }
            let capacity = (((address >> 24) & 3) + 1) as usize * 256;
            let entry = reader.stream(address, capacity)?;
            verify_self_hash(&entry, 92)?;
            if u32_at(&entry, 0)? as usize & (table_len - 1) != bucket { return Err(TypelessImportError::Unsupported); }
            address = u32_at(&entry, 4)?;
            if u32_at(&entry, 20)? != 0 { continue; }
            let key_len = u32_at(&entry, 32)? as usize;
            // All supported dictionary keys fit in the entry. Other keys are not read.
            if key_len == 0 || key_len >= capacity - 96 || u32_at(&entry, 36)? != 0 { continue; }
            if entry[96 + key_len] != 0 { return Err(TypelessImportError::Unsupported); }
            if persistent_hash(&entry[96..96+key_len]) != u32_at(&entry, 0)? {
                return Err(TypelessImportError::Unsupported);
            }
            let Ok(key) = std::str::from_utf8(&entry[96..96+key_len]) else { continue; };
            if !is_full_dictionary_key(key) { continue; }
            if u32_at(&entry, 72)? != 0 { return Err(TypelessImportError::Unsupported); }
            let rankings = reader.stream(u32_at(&entry, 8)?, 36)?;
            verify_self_hash(&rankings, 32)?;
            if u32_at(&rankings, 24)? != entry_address {
                return Err(TypelessImportError::Unsupported);
            }
            if u32_at(&rankings, 28)? != 0 { return Err(TypelessImportError::Changed); }
            let info = reader.stream(u32_at(&entry, 56)?, u32_at(&entry, 40)? as usize)?;
            let (response_time, encoding) = response_metadata(&info)?;
            candidates.push(Candidate { response_time, encoding, body_address: u32_at(&entry, 60)?, body_size: u32_at(&entry, 44)? as usize });
        }
    }
    // Select newest BEFORE decoding/validating. A broken/incomplete new response
    // must not resurrect deleted words from an older, apparently complete result.
    let candidate = candidates.into_iter().max_by_key(|c| c.response_time).ok_or(TypelessImportError::Unavailable)?;
    let body = reader.stream(candidate.body_address, candidate.body_size)?;
    let decoded = match candidate.encoding.as_str() {
        "" | "identity" => body,
        "br" => {
            let mut bytes = Vec::new();
            brotli::Decompressor::new(body.as_slice(), 4096).take((MAX_DECODED + 1) as u64)
                .read_to_end(&mut bytes).map_err(|_| TypelessImportError::Unsupported)?;
            if bytes.len() > MAX_DECODED { return Err(TypelessImportError::TooLarge); }
            bytes
        }
        _ => return Err(TypelessImportError::Unsupported),
    };
    let terms = response_terms(&decoded)?;
    reader.verify()?;
    let micros = candidate.response_time.checked_sub(11_644_473_600_000_000).ok_or(TypelessImportError::Unsupported)?;
    let cached_at = chrono::DateTime::from_timestamp_micros(micros).map(|time| time.to_rfc3339());
    Ok(ImportedTerms { terms, cached_at })
}

fn is_full_dictionary_key(key: &str) -> bool {
    let Some(url) = key.strip_prefix("1/0/") else { return false; };
    let Some(query) = url.strip_prefix(ENDPOINT).and_then(|s| s.strip_prefix('?')) else { return false; };
    // All other params (auto/search/category/page/cursor) denote a partial view.
    let Some(size) = query.strip_prefix("size=") else { return false; };
    !size.is_empty() && size.bytes().all(|b| b.is_ascii_digit()) && size.parse::<usize>().is_ok_and(|n| n > 0 && n <= 10_000)
}

fn response_metadata(info: &[u8]) -> Result<(i64, String)> {
    if u32_at(info, 0)? as usize != info.len().checked_sub(4).ok_or(TypelessImportError::Unsupported)? { return Err(TypelessImportError::Unsupported); }
    let flags = u32_at(info, 4)?;
    if flags & 255 != 3 || flags & (1 << 12) != 0 { return Err(TypelessImportError::Incomplete); }
    let mut offset = 8;
    let extra = if flags & (1 << 31) != 0 { let value = u32_at(info, offset)?; offset += 4; value } else { 0 };
    // Chromium may additionally compress the persisted body independently of HTTP.
    if extra & (1 << 4) != 0 { return Err(TypelessImportError::Unsupported); }
    let response_time = i64_at(info, offset + 8)?;
    if response_time <= 0 { return Err(TypelessImportError::Unsupported); }
    offset += 16;
    if extra & (1 << 2) != 0 { offset += 8; }
    let length = u32_at(info, offset)? as usize;
    if length > 65_536 { return Err(TypelessImportError::TooLarge); }
    let headers = info.get(offset+4..offset+4+length).ok_or(TypelessImportError::Unsupported)?;
    let mut lines = headers.split(|byte| *byte == 0);
    let status = lines.next().ok_or(TypelessImportError::Unsupported)?;
    if status != b"HTTP/1.1 200" && status != b"HTTP/1.0 200" && !status.starts_with(b"HTTP/1.1 200 ") {
        return Err(TypelessImportError::Incomplete);
    }
    let mut encoding = None;
    let mut content_type = None;
    for line in lines {
        let Some(colon) = line.iter().position(|byte| *byte == b':') else { continue; };
        // Ignore cookies, credentials and all other headers; never decode/log them.
        if line[..colon].eq_ignore_ascii_case(b"content-encoding") {
            if encoding.is_some() { return Err(TypelessImportError::Unsupported); }
            encoding = Some(std::str::from_utf8(&line[colon+1..]).map_err(|_| TypelessImportError::Unsupported)?.trim().to_ascii_lowercase());
        } else if line[..colon].eq_ignore_ascii_case(b"content-type") {
            if content_type.is_some() { return Err(TypelessImportError::Unsupported); }
            content_type = Some(std::str::from_utf8(&line[colon+1..]).map_err(|_| TypelessImportError::Unsupported)?.trim().to_ascii_lowercase());
        }
    }
    if !content_type.as_deref().is_some_and(|value| value.split(';').next() == Some("application/json")) {
        return Err(TypelessImportError::Unsupported);
    }
    Ok((response_time, encoding.unwrap_or_default()))
}

#[derive(Deserialize)] struct DictionaryResponse { status: String, data: DictionaryData }
#[derive(Deserialize)] struct DictionaryData { words: Vec<Word>, total_count: usize }
#[derive(Deserialize)] struct Word { term: String }
fn response_terms(body: &[u8]) -> Result<Vec<String>> {
    if body.len() > MAX_DECODED { return Err(TypelessImportError::TooLarge); }
    let response: DictionaryResponse = serde_json::from_slice(body).map_err(|_| TypelessImportError::Unsupported)?;
    if response.status != "OK" { return Err(TypelessImportError::Incomplete); }
    if response.data.total_count != response.data.words.len() { return Err(TypelessImportError::Incomplete); }
    if response.data.words.is_empty() { return Err(TypelessImportError::Empty); }
    let terms: Vec<String> = response.data.words.into_iter().map(|word| word.term).collect();
    if terms.iter().any(|term| term.trim().is_empty() || term.contains(['\r', '\n', '\0'])) { return Err(TypelessImportError::Unsupported); }
    Ok(terms)
}

fn verify_self_hash(bytes: &[u8], offset: usize) -> Result<()> {
    let stored = u32_at(bytes, offset)?;
    // Chromium accepts zero for legacy blocks. Nonzero hashes cover metadata,
    // including body addresses, so stable corrupted pointers cannot be trusted.
    if stored != 0 && stored != persistent_hash(&bytes[..offset]) {
        return Err(TypelessImportError::Unsupported);
    }
    Ok(())
}

// SuperFastHash adaptation: Copyright (c) 2010, Paul Hsieh. BSD-3-Clause.
// Full license: docs/third-party/supplements/chromium-superfasthash-LICENSE.txt.
// Chromium base::PersistentHash is this algorithm, including signed trailing
// bytes. Every addition uses uint32 wrapping, matching its persisted format.
fn persistent_hash(bytes: &[u8]) -> u32 {
    if bytes.is_empty() { return 0; }
    let mut hash = bytes.len() as u32;
    let mut chunks = bytes.chunks_exact(4);
    for chunk in &mut chunks {
        hash = hash.wrapping_add(u16::from_le_bytes([chunk[0], chunk[1]]) as u32);
        let mixed = ((u16::from_le_bytes([chunk[2], chunk[3]]) as u32) << 11) ^ hash;
        hash = (hash << 16) ^ mixed;
        hash = hash.wrapping_add(hash >> 11);
    }
    let tail = chunks.remainder();
    match tail.len() {
        3 => {
            hash = hash.wrapping_add(u16::from_le_bytes([tail[0], tail[1]]) as u32);
            hash ^= hash << 16;
            hash ^= (tail[2] as i8 as i32 as u32) << 18;
            hash = hash.wrapping_add(hash >> 11);
        }
        2 => {
            hash = hash.wrapping_add(u16::from_le_bytes([tail[0], tail[1]]) as u32);
            hash ^= hash << 11;
            hash = hash.wrapping_add(hash >> 17);
        }
        1 => {
            hash = hash.wrapping_add(tail[0] as i8 as i32 as u32);
            hash ^= hash << 10;
            hash = hash.wrapping_add(hash >> 1);
        }
        _ => {}
    }
    hash ^= hash << 3;
    hash = hash.wrapping_add(hash >> 5);
    hash ^= hash << 4;
    hash = hash.wrapping_add(hash >> 17);
    hash ^= hash << 25;
    hash.wrapping_add(hash >> 6)
}

#[cfg(test)]
#[path = "typeless_dictionary_tests.rs"]
mod tests;
