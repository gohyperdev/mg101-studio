//! Rdzeń domenowy MG101 Studio — model kanoniczny (niezależny od urządzenia).
//!
//! E1: model kanoniczny + profil/katalog (dane) + codec offline z round-trip
//! 1:1. Warstwa jest device-agnostyczna (sterowana profilem) i kompilowalna do
//! wasm32. Codec kontenera `.mg101patch` i protokół MG-101 żyją w
//! `pack-nux-mg101` na bazie tych typów.

pub mod canonical;
pub mod device_profile;
pub mod effect_catalog;
pub mod error;
pub mod patch_record;
pub mod wal;

pub use canonical::{BlockState, CanonicalPatch};
pub use device_profile::{Block, Bpm, ByteRange, DeviceProfile, NamedField};
pub use effect_catalog::{EffectCatalog, Model, Module, Parameter};
pub use error::{PatchError, ProfileError};
pub use patch_record::{ByteDifference, PatchRecord};
pub use wal::{
    plan_recovery, sha256_hex, EntryState, InverseOperation, JournalStore, RecoveryAction,
    StagedMetadata, TransactionEntry, WalError,
};

/// Rola crate'u (znacznik zgodności — używany przez szkielet desktop).
pub const CRATE_ROLE: &str = "canonical-model";
