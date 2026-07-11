//! Kontrakt Device Packa — generyczna warstwa wsparcia urządzenia.
//!
//! Kluczowa zasada (ADR-0002, HLD Biblioteka/urządzenie §2): rdzeń, biblioteka
//! i silnik transferu **nie znają** MG-101. Operują na abstrakcyjnej topologii
//! pamięci deklarowanej przez Device Pack (dane) oraz na traitach protokołu
//! (kod packa). Dodanie nowego urządzenia = nowy pack, bez zmian w silniku.
//!
//! Na E0 utrwalone są typy topologii i trait [`DeviceStorage`]. Trait protokołu
//! (`read_slot`/`write_slot`/`read_bank`) i codec wchodzą w E1/E2.

use mg101_device_link::DeviceLink;

/// Identyfikator banku pamięci urządzenia (np. `"user"`, `"factory"`).
pub type BankId = String;

/// Adres slotu: para (bank, indeks). Uniwersalny dla dowolnej topologii —
/// dwa banki × 36 (MG-101), płaska lista 128, banki A/B/C/D itd.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SlotAddr {
    /// Bank, do którego należy slot.
    pub bank: BankId,
    /// Indeks slotu w banku (bazowany wg `BankInfo::index_base`).
    pub index: u16,
}

/// Opis pojedynczego banku (z profilu Device Packa — dane, nie kod).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BankInfo {
    /// Identyfikator banku.
    pub id: BankId,
    /// Etykieta prezentacyjna (np. "User").
    pub label: String,
    /// Liczba slotów w banku.
    pub slots: u16,
    /// Bazowy indeks (0 lub 1).
    pub index_base: u16,
    /// Czy bank jest zapisywalny (Factory MG-101 = false).
    pub writable: bool,
    /// Czy sloty można przestawiać (reorder).
    pub reorderable: bool,
    /// Czy slot można wyczyścić/skasować.
    pub erasable: bool,
}

/// Limity zapisu deklarowane przez profil.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StorageLimits {
    /// Maksymalna liczba zapisywalnych slotów łącznie (None = bez limitu).
    pub max_writable_total: Option<u16>,
    /// Limit per bank (id banku → maks. liczba zapisywalnych slotów).
    pub per_bank: std::collections::BTreeMap<BankId, u16>,
}

/// Mapa zajętości slotów (do wyliczania wolnych miejsc przy transferze).
pub type SlotMap = std::collections::HashSet<SlotAddr>;

/// Generyczny opis topologii pamięci urządzenia. Silnik transferu pyta wyłącznie
/// przez ten trait — nie zna konkretnego sprzętu.
pub trait DeviceStorage {
    /// Banki urządzenia (z profilu).
    fn banks(&self) -> &[BankInfo];

    /// Czy dany bank jest zapisywalny.
    fn is_writable(&self, bank: &BankId) -> bool {
        self.banks()
            .iter()
            .find(|b| &b.id == bank)
            .map(|b| b.writable)
            .unwrap_or(false)
    }

    /// Limity zapisu.
    fn limits(&self) -> &StorageLimits;

    /// Wolne (niezajęte) sloty zapisywalnego banku, w kolejności indeksów.
    fn free_slots(&self, bank: &BankId, occupied: &SlotMap) -> Vec<SlotAddr> {
        let Some(info) = self.banks().iter().find(|b| &b.id == bank) else {
            return Vec::new();
        };
        if !info.writable {
            return Vec::new();
        }
        (0..info.slots)
            .map(|i| SlotAddr {
                bank: bank.clone(),
                index: info.index_base + i,
            })
            .filter(|a| !occupied.contains(a))
            .collect()
    }
}

/// Trait protokołu urządzenia (odczyt/zapis slotów). Sygnatury minimalne na E0;
/// pełna implementacja MG-101 (ramki `09/0B 00 <idx>`, `0B 01 <slot>`) w E2.
pub trait DeviceProtocol {
    /// Odczyt surowego blobu slotu z urządzenia.
    fn read_slot(
        &self,
        link: &mut dyn DeviceLink,
        addr: &SlotAddr,
    ) -> Result<Vec<u8>, ProtocolError>;
    /// Zapis surowego blobu do slotu urządzenia (operacja wrażliwa — pod kontrolą).
    fn write_slot(
        &self,
        link: &mut dyn DeviceLink,
        addr: &SlotAddr,
        blob: &[u8],
    ) -> Result<(), ProtocolError>;
    /// Odczyt całego banku (bulk dump). Domyślnie sekwencyjnie przez `read_slot`.
    fn read_bank(
        &self,
        link: &mut dyn DeviceLink,
        bank: &BankId,
        info: &BankInfo,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        (0..info.slots)
            .map(|i| {
                self.read_slot(
                    link,
                    &SlotAddr {
                        bank: bank.clone(),
                        index: info.index_base + i,
                    },
                )
            })
            .collect()
    }
}

/// Błąd protokołu urządzenia.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// Slot poza zakresem / bank nieznany.
    BadAddr(SlotAddr),
    /// Bank tylko do odczytu.
    ReadOnly(BankId),
    /// Nieoczekiwana/niekompletna odpowiedź urządzenia.
    BadResponse(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TwoBank;
    impl DeviceStorage for TwoBank {
        fn banks(&self) -> &[BankInfo] {
            // MG-101: User(zapis) + Factory(ro), po 36.
            static BANKS: std::sync::OnceLock<Vec<BankInfo>> = std::sync::OnceLock::new();
            BANKS.get_or_init(|| {
                vec![
                    BankInfo {
                        id: "user".into(),
                        label: "User".into(),
                        slots: 36,
                        index_base: 0,
                        writable: true,
                        reorderable: true,
                        erasable: false,
                    },
                    BankInfo {
                        id: "factory".into(),
                        label: "Factory".into(),
                        slots: 36,
                        index_base: 0,
                        writable: false,
                        reorderable: false,
                        erasable: false,
                    },
                ]
            })
        }
        fn limits(&self) -> &StorageLimits {
            static L: std::sync::OnceLock<StorageLimits> = std::sync::OnceLock::new();
            L.get_or_init(|| StorageLimits {
                max_writable_total: Some(36),
                per_bank: std::collections::BTreeMap::from([("user".to_string(), 36u16)]),
            })
        }
    }

    #[test]
    fn factory_is_not_writable_and_has_no_free_slots() {
        let s = TwoBank;
        assert!(s.is_writable(&"user".to_string()));
        assert!(!s.is_writable(&"factory".to_string()));
        assert!(s
            .free_slots(&"factory".to_string(), &SlotMap::new())
            .is_empty());
    }

    #[test]
    fn user_free_slots_exclude_occupied() {
        let s = TwoBank;
        let mut occ = SlotMap::new();
        occ.insert(SlotAddr {
            bank: "user".into(),
            index: 0,
        });
        let free = s.free_slots(&"user".to_string(), &occ);
        assert_eq!(free.len(), 35);
        assert_eq!(
            free[0],
            SlotAddr {
                bank: "user".into(),
                index: 1
            }
        );
    }
}
