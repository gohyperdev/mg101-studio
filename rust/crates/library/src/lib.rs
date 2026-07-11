//! Biblioteka patchy: store, tagi, grupy, fingerprint, provenance, stan trojdrozny (HLD sekcje 3-4). E4.
//!
//! E0: szkielet z realnym grafem zależności. Implementacja w kolejnych epikach.

/// Rola crate'u (zastępowana implementacją w docelowym epiku).
pub const CRATE_ROLE: &str = "library";

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {
        assert_eq!(super::CRATE_ROLE, "library");
    }
}
