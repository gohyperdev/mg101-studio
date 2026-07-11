//! Silnik transferu Biblioteka ↔ urządzenie (HLD §5). Generyczny — zależny tylko
//! od [`DeviceStorage`]/[`DeviceProtocol`] (kontrakt packa) i [`LibraryStore`].
//!
//! Podział: **planowanie** ([`plan`]) czyste i testowalne bez sprzętu; **wykonanie**
//! ([`exec`]) używa protokołu i transportu za traitami (testowane mockiem).
//! Zasada bezpieczeństwa zapisu (`mg101-probe`): odczyt swobodny, zapis pod
//! kontrolą — jawny plan, walidacja całościowa, WAL, `expectedRevision` na slocie.

mod exec;
mod plan;
mod sync_plan;

pub use exec::{execute_push, pull_slot, ExecError, PushOutcome, SlotWriteContext};
pub use plan::{plan_push, Placement, PlanError, PlannedWrite, TransferPlan};
pub use sync_plan::{decide_sync, ConflictPolicy, SyncAction};
