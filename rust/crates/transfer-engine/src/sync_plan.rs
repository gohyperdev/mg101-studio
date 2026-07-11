//! Decyzja synchronizacji (HLD §5: „Sync wg stanu trójdrożnego z decyzją per
//! konflikt"). Czysta funkcja mapująca [`SyncState`] (silnik E4) na rekomendowaną
//! akcję transferu, przy zadanej **polityce** rozwiązywania konfliktów. Nic nie
//! zapisuje — produkuje plan/rekomendację do zatwierdzenia (dry-run).

use mg101_library::SyncState;

/// Polityka rozwiązywania konfliktów przy sync (wybór użytkownika/UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    /// Urządzenie jest źródłem prawdy — przy rozjeździe importuj do Biblioteki.
    PreferDevice,
    /// Biblioteka jest źródłem prawdy — przy rozjeździe wyślij ponownie na slot.
    PreferLibrary,
    /// Nie ruszaj rozjazdów — tylko oznacz do ręcznej decyzji.
    Manual,
}

/// Rekomendowana akcja dla slotu po analizie stanu i polityki.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncAction {
    /// Nic do zrobienia (zgodne).
    None,
    /// Wyślij patch z Biblioteki na slot (Push).
    PushToSlot { patch_id: String },
    /// Zaimportuj/aktualizuj Bibliotekę ze slotu (Pull).
    PullToLibrary { patch_id: Option<String> },
    /// Konflikt do ręcznego rozstrzygnięcia (slot i Biblioteka się rozeszły).
    ManualResolve { patch_id: String },
}

/// Mapuje stan slotu na akcję sync wg polityki. `writable=false` (Factory) nigdy
/// nie prowadzi do zapisu na urządzenie — najwyżej import do Biblioteki.
pub fn decide_sync(state: &SyncState, writable: bool, policy: ConflictPolicy) -> SyncAction {
    match state {
        SyncState::InSync { .. } => SyncAction::None,
        SyncState::Empty => SyncAction::None, // pusty slot to cel jawnego push, nie auto-sync
        SyncState::DeviceOnly => SyncAction::PullToLibrary { patch_id: None },
        SyncState::LibraryNewer { patch_id } => {
            if writable {
                SyncAction::PushToSlot {
                    patch_id: patch_id.clone(),
                }
            } else {
                // Factory nie da się nadpisać — Biblioteka i tak jest nowsza.
                SyncAction::None
            }
        }
        SyncState::DeviceModified { patch_id } => match policy {
            ConflictPolicy::PreferDevice => SyncAction::PullToLibrary {
                patch_id: Some(patch_id.clone()),
            },
            ConflictPolicy::PreferLibrary if writable => SyncAction::PushToSlot {
                patch_id: patch_id.clone(),
            },
            ConflictPolicy::PreferLibrary => SyncAction::PullToLibrary {
                patch_id: Some(patch_id.clone()),
            },
            ConflictPolicy::Manual => SyncAction::ManualResolve {
                patch_id: patch_id.clone(),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dm() -> SyncState {
        SyncState::DeviceModified {
            patch_id: "p1".into(),
        }
    }

    #[test]
    fn in_sync_and_empty_do_nothing() {
        assert_eq!(
            decide_sync(
                &SyncState::InSync {
                    patch_id: "p".into()
                },
                true,
                ConflictPolicy::Manual
            ),
            SyncAction::None
        );
        assert_eq!(
            decide_sync(&SyncState::Empty, true, ConflictPolicy::PreferLibrary),
            SyncAction::None
        );
    }

    #[test]
    fn device_only_imports() {
        assert_eq!(
            decide_sync(&SyncState::DeviceOnly, false, ConflictPolicy::Manual),
            SyncAction::PullToLibrary { patch_id: None }
        );
    }

    #[test]
    fn library_newer_pushes_only_to_writable() {
        let s = SyncState::LibraryNewer {
            patch_id: "p1".into(),
        };
        assert_eq!(
            decide_sync(&s, true, ConflictPolicy::Manual),
            SyncAction::PushToSlot {
                patch_id: "p1".into()
            }
        );
        // Factory: brak zapisu.
        assert_eq!(
            decide_sync(&s, false, ConflictPolicy::Manual),
            SyncAction::None
        );
    }

    #[test]
    fn device_modified_follows_policy() {
        assert_eq!(
            decide_sync(&dm(), true, ConflictPolicy::PreferDevice),
            SyncAction::PullToLibrary {
                patch_id: Some("p1".into())
            }
        );
        assert_eq!(
            decide_sync(&dm(), true, ConflictPolicy::PreferLibrary),
            SyncAction::PushToSlot {
                patch_id: "p1".into()
            }
        );
        assert_eq!(
            decide_sync(&dm(), true, ConflictPolicy::Manual),
            SyncAction::ManualResolve {
                patch_id: "p1".into()
            }
        );
        // PreferLibrary na Factory nie może pisać → import.
        assert_eq!(
            decide_sync(&dm(), false, ConflictPolicy::PreferLibrary),
            SyncAction::PullToLibrary {
                patch_id: Some("p1".into())
            }
        );
    }
}
