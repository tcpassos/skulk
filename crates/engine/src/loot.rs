//! Loot stores. [`MemLoot`] is a volatile in-memory store for tests/dev;
//! [`RedbLoot`] is the persistent, atomic on-disk store (single file, ACID) that
//! survives reboots and power loss.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use redb::{Database, ReadableTable, TableDefinition};

use contract::{LootKind, LootQuery};
use module_sdk::{LootEntry, LootError, LootSink};

/// Volatile in-memory loot store — for tests and dev only.
#[derive(Default)]
pub struct MemLoot {
    inner: Mutex<HashMap<String, (LootKind, usize)>>,
}

#[async_trait]
impl LootSink for MemLoot {
    async fn put(&self, kind: LootKind, key: &str, bytes: Vec<u8>) -> Result<(), LootError> {
        self.inner.lock().unwrap().insert(key.to_string(), (kind, bytes.len()));
        Ok(())
    }

    async fn query(&self, query: &LootQuery) -> Result<Vec<LootEntry>, LootError> {
        let map = self.inner.lock().unwrap();
        Ok(filter(
            map.iter().map(|(k, (kind, size))| (k.clone(), *kind, *size as u64)),
            query,
        ))
    }

    async fn clear(&self) -> Result<(), LootError> {
        self.inner.lock().unwrap().clear();
        Ok(())
    }
}

const LOOT: TableDefinition<&str, &[u8]> = TableDefinition::new("loot");

/// Persistent, atomic loot store backed by redb.
///
/// Value layout on disk is `[kind_byte] ++ payload`, so a single table holds both
/// the category and the bytes without an extra codec dependency.
pub struct RedbLoot {
    db: Arc<Database>,
}

impl RedbLoot {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, LootError> {
        let db = Database::create(path).map_err(to_loot_err)?;
        // Materialize the table so a fresh DB can be read from immediately.
        let write = db.begin_write().map_err(to_loot_err)?;
        {
            write.open_table(LOOT).map_err(to_loot_err)?;
        }
        write.commit().map_err(to_loot_err)?;
        Ok(Self { db: Arc::new(db) })
    }
}

#[async_trait]
impl LootSink for RedbLoot {
    async fn put(&self, kind: LootKind, key: &str, bytes: Vec<u8>) -> Result<(), LootError> {
        let db = self.db.clone();
        let key = key.to_string();
        let mut value = Vec::with_capacity(bytes.len() + 1);
        value.push(kind_to_u8(kind));
        value.extend_from_slice(&bytes);

        // redb is blocking; keep it off the async executor.
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let write = db.begin_write().map_err(|e| e.to_string())?;
            {
                let mut table = write.open_table(LOOT).map_err(|e| e.to_string())?;
                table.insert(key.as_str(), value.as_slice()).map_err(|e| e.to_string())?;
            }
            write.commit().map_err(|e| e.to_string())?;
            Ok(())
        })
        .await
        .map_err(|e| LootError(e.to_string()))?
        .map_err(LootError)
    }

    async fn query(&self, query: &LootQuery) -> Result<Vec<LootEntry>, LootError> {
        let db = self.db.clone();
        let query = query.clone();

        tokio::task::spawn_blocking(move || -> Result<Vec<LootEntry>, String> {
            let read = db.begin_read().map_err(|e| e.to_string())?;
            let table = read.open_table(LOOT).map_err(|e| e.to_string())?;
            let mut rows = Vec::new();
            for item in table.iter().map_err(|e| e.to_string())? {
                let (k, v) = item.map_err(|e| e.to_string())?;
                let key = k.value().to_string();
                let bytes = v.value();
                let kind = u8_to_kind(bytes.first().copied().unwrap_or(u8::MAX));
                let size = bytes.len().saturating_sub(1) as u64;
                rows.push((key, kind, size));
            }
            Ok(filter(rows.into_iter(), &query))
        })
        .await
        .map_err(|e| LootError(e.to_string()))?
        .map_err(LootError)
    }

    async fn clear(&self) -> Result<(), LootError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let write = db.begin_write().map_err(|e| e.to_string())?;
            {
                write.delete_table(LOOT).map_err(|e| e.to_string())?;
                // Recreate the (now empty) table so subsequent reads still succeed.
                write.open_table(LOOT).map_err(|e| e.to_string())?;
            }
            write.commit().map_err(|e| e.to_string())?;
            Ok(())
        })
        .await
        .map_err(|e| LootError(e.to_string()))?
        .map_err(LootError)
    }
}

fn to_loot_err<E: std::fmt::Display>(e: E) -> LootError {
    LootError(e.to_string())
}

/// Apply prefix/kind/limit filtering, sorted by key, shared by both stores.
fn filter(
    items: impl Iterator<Item = (String, LootKind, u64)>,
    query: &LootQuery,
) -> Vec<LootEntry> {
    let mut out: Vec<LootEntry> = items
        .filter(|(key, kind, _)| {
            query.prefix.as_ref().map_or(true, |p| key.starts_with(p))
                && query.kind.map_or(true, |want| want == *kind)
        })
        .map(|(key, kind, size)| LootEntry { key, kind, size })
        .collect();
    out.sort_by(|a, b| a.key.cmp(&b.key));
    if let Some(limit) = query.limit {
        out.truncate(limit as usize);
    }
    out
}

fn kind_to_u8(kind: LootKind) -> u8 {
    match kind {
        LootKind::Hash => 0,
        LootKind::Handshake => 1,
        LootKind::Credential => 2,
        LootKind::Pcap => 3,
        LootKind::Telemetry => 4,
        LootKind::File => 5,
        LootKind::Other => 6,
    }
}

fn u8_to_kind(byte: u8) -> LootKind {
    match byte {
        0 => LootKind::Hash,
        1 => LootKind::Handshake,
        2 => LootKind::Credential,
        3 => LootKind::Pcap,
        4 => LootKind::Telemetry,
        5 => LootKind::File,
        _ => LootKind::Other,
    }
}
