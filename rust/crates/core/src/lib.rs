//! Rdzeń domenowy MG101 Studio — model kanoniczny (niezależny od urządzenia).
//!
//! E0: szkielet. E1 wypełnia model kanoniczny (patch/bloki/parametry/IR),
//! silnik diff i rewizje; codec MG-101 i round-trip 1:1 przeciw plikom v1
//! żyją w `pack-nux-mg101`, testowane z użyciem tego modelu.

/// Znacznik obecności rdzenia (zastępowany właściwym modelem w E1).
pub const CRATE_ROLE: &str = "canonical-model";

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {
        assert_eq!(super::CRATE_ROLE, "canonical-model");
    }
}
