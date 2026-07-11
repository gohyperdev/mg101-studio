//! Device Pack MG-101: profile (dane) + codec (189B rekord <-> kanonik <-> .mg101patch) + protocol (SysEx). Round-trip 1:1 w E1/E2.
//!
//! E0: szkielet z realnym grafem zależności. Implementacja w kolejnych epikach.

/// Rola crate'u (zastępowana implementacją w docelowym epiku).
pub const CRATE_ROLE: &str = "device-pack-nux-mg101";

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {
        assert_eq!(super::CRATE_ROLE, "device-pack-nux-mg101");
    }
}
