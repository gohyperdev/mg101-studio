//! Dziennik transakcji (WAL) — port `WAL.swift` z v1.
//!
//! Podział (wasm-readiness): **logika** (typy wpisów, hash SHA256, algorytm
//! crash-recovery, plan revert) jest czysta i testowalna, **skład** (append-only
//! plik z fsync) jest natywny za traitem [`JournalStore`] (jak transport/backend
//! w `device-link`). Semantyka bajtowa i stanów 1:1 ze Swiftem.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Identyfikator patcha (UUID lub `factory-NN`, jak w v1).
pub type PatchId = String;

/// SHA256 danych w hex (małe litery) — odpowiednik `Data.sha256Hex()`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Stan wpisu: dwufazowo `prepared` → `committed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryState {
    Prepared,
    Committed,
}

/// Metadane patcha przeniesionego do poczekalni (soft-delete).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedMetadata {
    pub origin: String,
    pub source_name: String,
    pub revision: i64,
    pub file_name: String,
}

/// Operacja odwrotna (do cofnięcia efektu wpisu).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InverseOperation {
    /// Przywróć bajty patcha.
    RestoreBytes { patch_id: PatchId, blob: Vec<u8> },
    /// Usuń patch (cofnięcie utworzenia/duplikatu/importu).
    RemovePatch { patch_id: PatchId },
    /// Przywróć patch z poczekalni (cofnięcie soft-delete).
    RestoreFromStaging {
        patch_id: PatchId,
        meta: StagedMetadata,
    },
}

impl InverseOperation {
    /// Patch, którego dotyczy operacja.
    pub fn patch_id(&self) -> &PatchId {
        match self {
            InverseOperation::RestoreBytes { patch_id, .. }
            | InverseOperation::RemovePatch { patch_id }
            | InverseOperation::RestoreFromStaging { patch_id, .. } => patch_id,
        }
    }
}

/// Pojedynczy wpis dziennika.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionEntry {
    pub sequence: u64,
    /// Znacznik czasu (ms epoch) — dostarczany przez wywołującego (wasm-safe).
    pub timestamp_ms: i64,
    pub tool_name: String,
    pub patch_id: PatchId,
    pub revision_before: i64,
    pub inverse: InverseOperation,
    /// SHA256 (hex) stanu patcha PRZED operacją.
    pub before_hash: String,
    /// SHA256 (hex) stanu patcha PO operacji.
    pub after_hash: String,
    pub state: EntryState,
}

/// Skład dziennika (append-only). Natywnie [`FileJournal`]; w wasm inny backend.
pub trait JournalStore {
    fn append(&self, entry: &TransactionEntry) -> Result<(), WalError>;
    fn load(&self) -> Result<Vec<TransactionEntry>, WalError>;
    fn rewrite(&self, entries: &[TransactionEntry]) -> Result<(), WalError>;
}

/// Błąd WAL.
#[derive(Debug)]
pub enum WalError {
    /// Błąd we/wy składu.
    Io(String),
    /// Błąd (de)serializacji.
    Serde(String),
    /// Konflikt hasha — stan na dysku nie zgadza się z oczekiwanym (ręczna edycja).
    /// Konstruowany przez strażnika `revertLastAgentAction` (wchodzi w E4 wraz
    /// z portem StudioState; wariant zadeklarowany tu, bo należy do słownika WAL).
    HashConflict { patch_id: PatchId },
}

impl std::fmt::Display for WalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WalError::Io(m) => write!(f, "WAL io: {m}"),
            WalError::Serde(m) => write!(f, "WAL serde: {m}"),
            WalError::HashConflict { patch_id } => {
                write!(f, "WAL konflikt hasha dla patcha {patch_id}")
            }
        }
    }
}

impl std::error::Error for WalError {}

// --- Czysta logika (recovery / revert) ---

/// Akcja odzyskiwania dla zawieszonego wpisu (prepared bez committed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Nic się nie stało (stan == before) — wpis do porzucenia.
    Drop { sequence: u64 },
    /// Operacja się wykonała (stan == after) — awansuj do committed.
    Promote { sequence: u64 },
    /// Stan pośredni/nieznany — cofnij przez inverse, potem porzuć.
    Rollback {
        sequence: u64,
        inverse: InverseOperation,
    },
}

/// Zwraca zawieszone wpisy `prepared` (sekwencje bez odpowiadającego `committed`).
pub fn dangling(entries: &[TransactionEntry]) -> Vec<&TransactionEntry> {
    let committed: std::collections::HashSet<u64> = entries
        .iter()
        .filter(|e| e.state == EntryState::Committed)
        .map(|e| e.sequence)
        .collect();
    entries
        .iter()
        .filter(|e| e.state == EntryState::Prepared && !committed.contains(&e.sequence))
        .collect()
}

/// Plan odzyskiwania po awarii — wierny port `recoverDanglingTransactions`.
/// `hash_of` zwraca bieżący SHA256 (hex) patcha na dysku (lub `None`, gdy pliku
/// brak). Jak w v1 brak pliku traktujemy jak pusty hash `""` (sentinel używany
/// dla świeżo tworzonych patchy `before=""` i usuwanych `after=""`), więc
/// recovery po przerwanym create/delete daje Drop/Promote, nie Rollback.
/// Plan sortowany malejąco po `sequence` (najnowsze cofane pierwsze — parzystość
/// kolejności inwersów przy wielu zawieszonych wpisach tego samego patcha).
pub fn plan_recovery<F>(entries: &[TransactionEntry], hash_of: F) -> Vec<RecoveryAction>
where
    F: Fn(&PatchId) -> Option<String>,
{
    let mut dangling = dangling(entries);
    dangling.sort_by(|a, b| b.sequence.cmp(&a.sequence));
    dangling
        .into_iter()
        .map(|e| {
            let current = hash_of(&e.patch_id).unwrap_or_default();
            if current == e.before_hash {
                RecoveryAction::Drop {
                    sequence: e.sequence,
                }
            } else if current == e.after_hash {
                RecoveryAction::Promote {
                    sequence: e.sequence,
                }
            } else {
                RecoveryAction::Rollback {
                    sequence: e.sequence,
                    inverse: e.inverse.clone(),
                }
            }
        })
        .collect()
}

/// Inwersy sesji do zastosowania przy `revertSession` — najnowsze pierwsze.
/// Obejmuje wpisy committed oraz zawieszone prepared. UWAGA: v1 cofa zawieszony
/// wpis tylko gdy jest ostatnią linią dziennika; tu cofamy wszystkie zawieszone
/// (bezpieczniejsze przy współbieżności). Świadome odstępstwo — do potwierdzenia
/// przy porcie StudioState w E4 (BACKLOG).
pub fn session_inverses(entries: &[TransactionEntry]) -> Vec<&InverseOperation> {
    let dangling_seqs: std::collections::HashSet<u64> =
        dangling(entries).iter().map(|e| e.sequence).collect();
    let mut relevant: Vec<&TransactionEntry> = entries
        .iter()
        .filter(|e| e.state == EntryState::Committed || dangling_seqs.contains(&e.sequence))
        .collect();
    relevant.sort_by(|a, b| b.sequence.cmp(&a.sequence)); // najnowsze pierwsze
    relevant.into_iter().map(|e| &e.inverse).collect()
}

/// Najnowszy wpis committed (do `revertLastAgentAction`).
pub fn last_committed(entries: &[TransactionEntry]) -> Option<&TransactionEntry> {
    entries
        .iter()
        .filter(|e| e.state == EntryState::Committed)
        .max_by_key(|e| e.sequence)
}

// --- Skład plikowy (natywny) ---

/// Plikowy dziennik append-only z fsync — port `WALJournal`.
#[cfg(not(target_arch = "wasm32"))]
pub struct FileJournal {
    path: std::path::PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
impl FileJournal {
    /// Dziennik pod wskazaną ścieżką.
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl JournalStore for FileJournal {
    fn append(&self, entry: &TransactionEntry) -> Result<(), WalError> {
        use std::io::Write;
        let line = serde_json::to_string(entry).map_err(|e| WalError::Serde(e.to_string()))?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| WalError::Io(e.to_string()))?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| WalError::Io(e.to_string()))?;
        f.write_all(line.as_bytes())
            .and_then(|_| f.write_all(b"\n"))
            .and_then(|_| f.sync_all()) // fsync
            .map_err(|e| WalError::Io(e.to_string()))
    }

    fn load(&self) -> Result<Vec<TransactionEntry>, WalError> {
        let content = match std::fs::read_to_string(&self.path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(WalError::Io(e.to_string())),
        };
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).map_err(|e| WalError::Serde(e.to_string())))
            .collect()
    }

    fn rewrite(&self, entries: &[TransactionEntry]) -> Result<(), WalError> {
        use std::io::Write;
        let mut content = String::new();
        for e in entries {
            content
                .push_str(&serde_json::to_string(e).map_err(|e| WalError::Serde(e.to_string()))?);
            content.push('\n');
        }
        // Atomowo (jak v1 `atomically: true`): zapis do pliku tymczasowego obok
        // celu, fsync, potem rename. Awaria w trakcie zostawia nietknięty stary
        // dziennik zamiast obciętego/uszkodzonego JSONL, którego load() nie odczyta.
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| WalError::Io(e.to_string()))?;
        }
        let tmp = self.path.with_extension("jsonl.tmp");
        {
            let mut f = std::fs::File::create(&tmp).map_err(|e| WalError::Io(e.to_string()))?;
            f.write_all(content.as_bytes())
                .and_then(|_| f.sync_all())
                .map_err(|e| WalError::Io(e.to_string()))?;
        }
        std::fs::rename(&tmp, &self.path).map_err(|e| WalError::Io(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(seq: u64, state: EntryState, before: &str, after: &str) -> TransactionEntry {
        TransactionEntry {
            sequence: seq,
            timestamp_ms: 1_000 + seq as i64,
            tool_name: "set_bpm".into(),
            patch_id: format!("patch-{seq}"),
            revision_before: 1,
            inverse: InverseOperation::RestoreBytes {
                patch_id: format!("patch-{seq}"),
                blob: vec![seq as u8; 4],
            },
            before_hash: before.into(),
            after_hash: after.into(),
            state,
        }
    }

    #[test]
    fn sha256_matches_known_vector() {
        // SHA256("") — znany wektor.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn dangling_finds_prepared_without_committed() {
        let entries = vec![
            entry(1, EntryState::Prepared, "a", "b"),
            entry(1, EntryState::Committed, "a", "b"),
            entry(2, EntryState::Prepared, "c", "d"), // zawieszony
        ];
        let d = dangling(&entries);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].sequence, 2);
    }

    #[test]
    fn recovery_drop_promote_rollback() {
        let entries = vec![
            entry(1, EntryState::Prepared, "before1", "after1"),
            entry(2, EntryState::Prepared, "before2", "after2"),
            entry(3, EntryState::Prepared, "before3", "after3"),
        ];
        // patch-1 nadal == before (nic się nie stało) → Drop
        // patch-2 == after (zapis się wykonał) → Promote
        // patch-3 stan pośredni → Rollback
        let plan = plan_recovery(&entries, |pid| match pid.as_str() {
            "patch-1" => Some("before1".into()),
            "patch-2" => Some("after2".into()),
            "patch-3" => Some("xxx".into()),
            _ => None,
        });
        // Plan sortowany malejąco po sequence (najnowsze pierwsze).
        assert_eq!(plan.len(), 3);
        assert!(matches!(
            plan[0],
            RecoveryAction::Rollback { sequence: 3, .. }
        ));
        assert!(matches!(plan[1], RecoveryAction::Promote { sequence: 2 }));
        assert!(matches!(plan[2], RecoveryAction::Drop { sequence: 1 }));
    }

    #[test]
    fn recovery_uses_empty_sentinel_when_file_missing() {
        // Przerwany DELETE: after_hash="" (patch znika). Plik już usunięty →
        // hash_of zwraca None. Sentinel "" musi dać Promote (dokończ usunięcie),
        // NIE Rollback (który wskrzesiłby patch). Regres wykryty w review E3.
        let mut del = entry(5, EntryState::Prepared, "before5", "");
        del.inverse = InverseOperation::RestoreFromStaging {
            patch_id: "patch-5".into(),
            meta: StagedMetadata {
                origin: "user".into(),
                source_name: "X".into(),
                revision: 1,
                file_name: "x.bin".into(),
            },
        };
        let plan = plan_recovery(&[del], |_| None);
        assert!(matches!(plan[0], RecoveryAction::Promote { sequence: 5 }));

        // Przerwany CREATE: before_hash="" (patch nie istniał). Plik nie powstał →
        // None. Sentinel "" musi dać Drop (nic się nie stało), nie Rollback.
        let create = entry(6, EntryState::Prepared, "", "after6");
        let plan = plan_recovery(&[create], |_| None);
        assert!(matches!(plan[0], RecoveryAction::Drop { sequence: 6 }));
    }

    #[test]
    fn session_inverses_are_newest_first() {
        let entries = vec![
            entry(1, EntryState::Committed, "a", "b"),
            entry(2, EntryState::Committed, "b", "c"),
            entry(3, EntryState::Prepared, "c", "d"), // zawieszony też się liczy
        ];
        let inv = session_inverses(&entries);
        assert_eq!(inv.len(), 3);
        assert_eq!(inv[0].patch_id(), "patch-3"); // najnowszy pierwszy
        assert_eq!(inv[2].patch_id(), "patch-1");
    }

    #[test]
    fn last_committed_ignores_prepared() {
        let entries = vec![
            entry(1, EntryState::Committed, "a", "b"),
            entry(2, EntryState::Prepared, "b", "c"),
        ];
        assert_eq!(last_committed(&entries).unwrap().sequence, 1);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn file_journal_append_load_rewrite_roundtrip() {
        let path = std::env::temp_dir().join(format!("mg101_wal_{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let j = FileJournal::new(&path);
        assert!(j.load().unwrap().is_empty());

        j.append(&entry(1, EntryState::Prepared, "a", "b")).unwrap();
        j.append(&entry(1, EntryState::Committed, "a", "b"))
            .unwrap();
        let loaded = j.load().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[1].state, EntryState::Committed);
        // Round-trip pól przez serde.
        assert_eq!(loaded[0], entry(1, EntryState::Prepared, "a", "b"));

        // rewrite obcina.
        j.rewrite(&[]).unwrap();
        assert!(j.load().unwrap().is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn rewrite_replaces_atomically() {
        let path =
            std::env::temp_dir().join(format!("mg101_wal_atom_{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let j = FileJournal::new(&path);
        j.append(&entry(1, EntryState::Committed, "a", "b"))
            .unwrap();
        j.append(&entry(2, EntryState::Committed, "b", "c"))
            .unwrap();
        // Kompaktowanie do jednego wpisu — po rename plik jest odczytywalny i pełny.
        j.rewrite(&[entry(2, EntryState::Committed, "b", "c")])
            .unwrap();
        let loaded = j.load().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].sequence, 2);
        // Brak pozostawionego pliku tymczasowego.
        assert!(!path.with_extension("jsonl.tmp").exists());
        let _ = std::fs::remove_file(&path);
    }
}
