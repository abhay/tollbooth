use axum::http::HeaderMap;
use spl_tollbooth_core::config::ProtocolsConfig;
use spl_tollbooth_core::types::ProtocolKind;

/// Negotiate the payment protocol from request headers.
/// MPP is the only supported protocol.
pub fn negotiate_protocol(_headers: &HeaderMap, _config: &ProtocolsConfig) -> ProtocolKind {
    ProtocolKind::Mpp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_returns_mpp() {
        let headers = HeaderMap::new();
        let config = ProtocolsConfig { mpp: true };
        assert_eq!(negotiate_protocol(&headers, &config), ProtocolKind::Mpp);
    }
}
