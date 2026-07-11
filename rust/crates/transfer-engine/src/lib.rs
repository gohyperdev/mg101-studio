//! Silnik transferu Biblioteka<->urzadzenie: push/pull/sync, transfer grupowy, limity, WAL (HLD sekcja 5). E5.
//!
//! E0: szkielet z realnym grafem zależności. Implementacja w kolejnych epikach.

/// Rola crate'u (zastępowana implementacją w docelowym epiku).
pub const CRATE_ROLE: &str = "transfer-engine";

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {
        assert_eq!(super::CRATE_ROLE, "transfer-engine");
    }
}
