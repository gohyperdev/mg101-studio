//! Trwały store Biblioteki na SQLite (desktop; HLD §3).
//!
//! Implementuje ten sam kontrakt [`LibraryStore`] co [`MemoryStore`], więc silniki
//! (sync, transfer E5) i UI działają na nim bez zmian. Patch/grupa/link są
//! przechowywane jako dokumenty JSON (bajty święte + metadane bez straty); kolumny
//! pochodne (`revision`) wspierają kontrolę współbieżności. Zapytania po tagach
//! filtrują w pamięci (skala desktopu; indeks per-tag → BACKLOG).
//!
//! Wyłączony na wasm (`cfg`), gdzie store to inny backend.

use crate::store::{LibraryError, LibraryStore};
use crate::{DeviceSlotLink, Group, GroupId, LibraryPatch, PatchId, SlotAddr, TagId};
use rusqlite::{Connection, OptionalExtension};

/// Store SQLite. `open`/`open_in_memory` tworzą schemat, jeśli brak.
pub struct SqliteStore {
    conn: Connection,
}

fn map_sqlite(e: rusqlite::Error) -> LibraryError {
    // Błędy silnika składu mapujemy na NotFound z opisem — warstwa wyżej i tak
    // traktuje je jako awarię we/wy; szczegół w treści.
    LibraryError::NotFound(format!("sqlite: {e}"))
}

fn map_json(e: serde_json::Error) -> LibraryError {
    LibraryError::NotFound(format!("json: {e}"))
}

impl SqliteStore {
    /// Otwiera (lub tworzy) bazę pod ścieżką i inicjuje schemat.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, LibraryError> {
        let conn = Connection::open(path).map_err(map_sqlite)?;
        Self::init(conn)
    }

    /// Baza w pamięci (testy/prototyp) z tym samym schematem.
    pub fn open_in_memory() -> Result<Self, LibraryError> {
        let conn = Connection::open_in_memory().map_err(map_sqlite)?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self, LibraryError> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS patches (
                 id TEXT PRIMARY KEY,
                 revision INTEGER NOT NULL,
                 doc TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS groups (
                 id TEXT PRIMARY KEY,
                 doc TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS links (
                 serial TEXT NOT NULL,
                 bank TEXT NOT NULL,
                 idx INTEGER NOT NULL,
                 doc TEXT NOT NULL,
                 PRIMARY KEY (serial, bank, idx)
             );",
        )
        .map_err(map_sqlite)?;
        Ok(Self { conn })
    }

    fn write_patch(&self, patch: &LibraryPatch) -> Result<(), LibraryError> {
        let doc = serde_json::to_string(patch).map_err(map_json)?;
        self.conn
            .execute(
                "INSERT INTO patches (id, revision, doc) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET revision=?2, doc=?3",
                rusqlite::params![patch.id, patch.revision, doc],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    fn write_group(&self, group: &Group) -> Result<(), LibraryError> {
        let doc = serde_json::to_string(group).map_err(map_json)?;
        self.conn
            .execute(
                "INSERT INTO groups (id, doc) VALUES (?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET doc=?2",
                rusqlite::params![group.id, doc],
            )
            .map_err(map_sqlite)?;
        Ok(())
    }

    fn read_patch(&self, id: &PatchId) -> Result<Option<LibraryPatch>, LibraryError> {
        let doc: Option<String> = self
            .conn
            .query_row("SELECT doc FROM patches WHERE id=?1", [id], |r| r.get(0))
            .optional()
            .map_err(map_sqlite)?;
        match doc {
            Some(d) => Ok(Some(serde_json::from_str(&d).map_err(map_json)?)),
            None => Ok(None),
        }
    }
}

impl LibraryStore for SqliteStore {
    fn add(&mut self, patch: LibraryPatch) -> Result<(), LibraryError> {
        if self.read_patch(&patch.id)?.is_some() {
            return Err(LibraryError::Duplicate(patch.id));
        }
        self.write_patch(&patch)
    }

    fn get(&self, id: &PatchId) -> Option<LibraryPatch> {
        self.read_patch(id).ok().flatten()
    }

    fn update(
        &mut self,
        mut patch: LibraryPatch,
        expected_revision: u64,
    ) -> Result<u64, LibraryError> {
        let current = self
            .read_patch(&patch.id)?
            .ok_or_else(|| LibraryError::NotFound(patch.id.clone()))?;
        if current.revision != expected_revision {
            return Err(LibraryError::RevisionConflict {
                patch_id: patch.id,
                expected: expected_revision,
                actual: current.revision,
            });
        }
        let new_revision = current.revision + 1;
        patch.revision = new_revision;
        self.write_patch(&patch)?;
        Ok(new_revision)
    }

    fn remove(&mut self, id: &PatchId) -> Result<LibraryPatch, LibraryError> {
        let existing = self
            .read_patch(id)?
            .ok_or_else(|| LibraryError::NotFound(id.clone()))?;
        self.conn
            .execute("DELETE FROM patches WHERE id=?1", [id])
            .map_err(map_sqlite)?;
        // Sprzątanie przynależności do grup.
        for mut g in self.groups() {
            if g.members.contains(id) {
                g.remove(id);
                self.write_group(&g)?;
            }
        }
        Ok(existing)
    }

    fn all(&self) -> Vec<LibraryPatch> {
        let mut stmt = match self.conn.prepare("SELECT doc FROM patches ORDER BY id") {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map([], |r| r.get::<_, String>(0));
        let Ok(rows) = rows else { return Vec::new() };
        rows.filter_map(|r| r.ok())
            .filter_map(|d| serde_json::from_str(&d).ok())
            .collect()
    }

    fn add_tag(&mut self, id: &PatchId, tag: TagId) -> Result<(), LibraryError> {
        let mut p = self
            .read_patch(id)?
            .ok_or_else(|| LibraryError::NotFound(id.clone()))?;
        p.tags.insert(tag);
        self.write_patch(&p)
    }

    fn remove_tag(&mut self, id: &PatchId, tag: &TagId) -> Result<(), LibraryError> {
        let mut p = self
            .read_patch(id)?
            .ok_or_else(|| LibraryError::NotFound(id.clone()))?;
        p.tags.remove(tag);
        self.write_patch(&p)
    }

    fn by_tag(&self, tag: &TagId) -> Vec<LibraryPatch> {
        self.all()
            .into_iter()
            .filter(|p| p.tags.contains(tag))
            .collect()
    }

    fn create_group(&mut self, group: Group) -> Result<(), LibraryError> {
        if self.group(&group.id).is_some() {
            return Err(LibraryError::Duplicate(group.id));
        }
        self.write_group(&group)
    }

    fn group(&self, id: &GroupId) -> Option<Group> {
        let doc: Option<String> = self
            .conn
            .query_row("SELECT doc FROM groups WHERE id=?1", [id], |r| r.get(0))
            .optional()
            .ok()
            .flatten();
        doc.and_then(|d| serde_json::from_str(&d).ok())
    }

    fn groups(&self) -> Vec<Group> {
        let mut stmt = match self.conn.prepare("SELECT doc FROM groups ORDER BY id") {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map([], |r| r.get::<_, String>(0));
        let Ok(rows) = rows else { return Vec::new() };
        rows.filter_map(|r| r.ok())
            .filter_map(|d| serde_json::from_str(&d).ok())
            .collect()
    }

    fn delete_group(&mut self, id: &GroupId) -> Result<(), LibraryError> {
        let n = self
            .conn
            .execute("DELETE FROM groups WHERE id=?1", [id])
            .map_err(map_sqlite)?;
        if n == 0 {
            return Err(LibraryError::NotFound(id.clone()));
        }
        // Usuń przynależność z patchy.
        for mut p in self.all() {
            if p.groups.remove(id) {
                self.write_patch(&p)?;
            }
        }
        Ok(())
    }

    fn add_to_group(&mut self, group: &GroupId, patch: &PatchId) -> Result<(), LibraryError> {
        let mut p = self
            .read_patch(patch)?
            .ok_or_else(|| LibraryError::NotFound(patch.clone()))?;
        let mut g = self
            .group(group)
            .ok_or_else(|| LibraryError::NotFound(group.clone()))?;
        g.push_unique(patch.clone());
        p.groups.insert(group.clone());
        self.write_group(&g)?;
        self.write_patch(&p)
    }

    fn remove_from_group(&mut self, group: &GroupId, patch: &PatchId) -> Result<(), LibraryError> {
        let mut g = self
            .group(group)
            .ok_or_else(|| LibraryError::NotFound(group.clone()))?;
        g.remove(patch);
        self.write_group(&g)?;
        if let Some(mut p) = self.read_patch(patch)? {
            if p.groups.remove(group) {
                self.write_patch(&p)?;
            }
        }
        Ok(())
    }

    fn reorder_group(&mut self, group: &GroupId, order: Vec<PatchId>) -> Result<(), LibraryError> {
        let mut g = self
            .group(group)
            .ok_or_else(|| LibraryError::NotFound(group.clone()))?;
        let same_set =
            order.len() == g.members.len() && order.iter().all(|p| g.members.contains(p));
        if !same_set {
            return Err(LibraryError::NotFound(format!(
                "kolejność nie jest permutacją członków grupy {group}"
            )));
        }
        g.members = order;
        self.write_group(&g)
    }

    fn record_link(&mut self, link: DeviceSlotLink) {
        if let Ok(doc) = serde_json::to_string(&link) {
            let _ = self.conn.execute(
                "INSERT INTO links (serial, bank, idx, doc) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(serial, bank, idx) DO UPDATE SET doc=?4",
                rusqlite::params![link.device_serial, link.slot.bank, link.slot.index, doc],
            );
        }
    }

    fn links_for_device(&self, serial: &str) -> Vec<DeviceSlotLink> {
        let mut stmt = match self.conn.prepare("SELECT doc FROM links WHERE serial=?1") {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map([serial], |r| r.get::<_, String>(0));
        let Ok(rows) = rows else { return Vec::new() };
        rows.filter_map(|r| r.ok())
            .filter_map(|d| serde_json::from_str(&d).ok())
            .collect()
    }

    fn link_for_slot(&self, serial: &str, slot: &SlotAddr) -> Option<DeviceSlotLink> {
        let doc: Option<String> = self
            .conn
            .query_row(
                "SELECT doc FROM links WHERE serial=?1 AND bank=?2 AND idx=?3",
                rusqlite::params![serial, slot.bank, slot.index],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten();
        doc.and_then(|d| serde_json::from_str(&d).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PatchOrigin;
    use std::collections::BTreeSet;

    fn patch(id: &str) -> LibraryPatch {
        LibraryPatch {
            id: id.into(),
            name: id.into(),
            blob: vec![9, 8, 7],
            origin: PatchOrigin::PulledFromDevice,
            device_id: "nux-mg101".into(),
            firmware: Some("1.0".into()),
            codec_version: "1".into(),
            content_hash: format!("c-{id}"),
            exact_hash: format!("e-{id}"),
            tags: BTreeSet::new(),
            groups: BTreeSet::new(),
            created_at: 1,
            updated_at: 1,
            revision: 1,
        }
    }

    #[test]
    fn crud_roundtrip_lossless() {
        let mut s = SqliteStore::open_in_memory().unwrap();
        s.add(patch("p1")).unwrap();
        let got = s.get(&"p1".into()).unwrap();
        assert_eq!(got, patch("p1")); // pełny round-trip (blob, firmware, hashe)
        assert_eq!(
            s.add(patch("p1")),
            Err(LibraryError::Duplicate("p1".into()))
        );
    }

    #[test]
    fn update_revision_and_persist() {
        let mut s = SqliteStore::open_in_memory().unwrap();
        s.add(patch("p1")).unwrap();
        let mut e = patch("p1");
        e.name = "nowa".into();
        assert!(matches!(
            s.update(e.clone(), 5),
            Err(LibraryError::RevisionConflict { actual: 1, .. })
        ));
        assert_eq!(s.update(e, 1).unwrap(), 2);
        assert_eq!(s.get(&"p1".into()).unwrap().name, "nowa");
    }

    #[test]
    fn tags_groups_links_persist_like_memory() {
        let mut s = SqliteStore::open_in_memory().unwrap();
        s.add(patch("p1")).unwrap();
        s.add(patch("p2")).unwrap();
        s.add_tag(&"p1".into(), "metal".into()).unwrap();
        assert_eq!(s.by_tag(&"metal".into()).len(), 1);

        s.create_group(Group {
            id: "g1".into(),
            name: "Koncert".into(),
            members: vec![],
        })
        .unwrap();
        s.add_to_group(&"g1".into(), &"p1".into()).unwrap();
        s.add_to_group(&"g1".into(), &"p2".into()).unwrap();
        assert_eq!(s.group(&"g1".into()).unwrap().members, vec!["p1", "p2"]);
        assert!(s.get(&"p1".into()).unwrap().groups.contains("g1"));
        s.reorder_group(&"g1".into(), vec!["p2".into(), "p1".into()])
            .unwrap();
        assert_eq!(s.group(&"g1".into()).unwrap().members, vec!["p2", "p1"]);

        let slot = SlotAddr {
            bank: "user".into(),
            index: 5,
        };
        s.record_link(DeviceSlotLink {
            device_serial: "SN1".into(),
            slot: slot.clone(),
            library_patch_id: "p1".into(),
            hash_at_transfer: "e-p1".into(),
            transferred_at: 100,
        });
        assert_eq!(
            s.link_for_slot("SN1", &slot).unwrap().library_patch_id,
            "p1"
        );
        // Usunięcie patcha czyści grupę.
        s.remove(&"p1".into()).unwrap();
        assert_eq!(s.group(&"g1".into()).unwrap().members, vec!["p2"]);
    }

    #[test]
    fn persists_across_reopen() {
        let path = std::env::temp_dir().join(format!("mg101_lib_{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let mut s = SqliteStore::open(&path).unwrap();
            s.add(patch("keep")).unwrap();
        }
        {
            let s = SqliteStore::open(&path).unwrap();
            assert_eq!(s.get(&"keep".into()).unwrap().name, "keep");
        }
        let _ = std::fs::remove_file(&path);
    }
}
