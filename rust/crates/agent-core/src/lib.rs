//! Rdzen agentowy: petla tool-calling, providerzy Anthropic/OpenAI, autoryzacje, sesje, liczenie kosztu (ADR-0001). E6.
//!
//! E0: szkielet z realnym grafem zależności. Implementacja w kolejnych epikach.

/// Rola crate'u (zastępowana implementacją w docelowym epiku).
pub const CRATE_ROLE: &str = "agent-core";

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {
        assert_eq!(super::CRATE_ROLE, "agent-core");
    }
}
