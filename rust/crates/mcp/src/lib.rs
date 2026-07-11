//! Serwer MCP (rmcp): stdio + Streamable HTTP na localhost; komendy z rejestru z anotacjami. E6.
//!
//! E0: szkielet z realnym grafem zależności. Implementacja w kolejnych epikach.

/// Rola crate'u (zastępowana implementacją w docelowym epiku).
pub const CRATE_ROLE: &str = "mcp";

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {
        assert_eq!(super::CRATE_ROLE, "mcp");
    }
}
