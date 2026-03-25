use axum::http::{HeaderMap, HeaderName, HeaderValue};
use spl_tollbooth_core::types::PaymentReceipt;

pub const X_TOLLBOOTH_VERIFIED: &str = "x-tollbooth-verified";
pub const X_TOLLBOOTH_AMOUNT: &str = "x-tollbooth-amount";
pub const X_TOLLBOOTH_UI_AMOUNT: &str = "x-tollbooth-ui-amount";
pub const X_TOLLBOOTH_PAYER: &str = "x-tollbooth-payer";
pub const X_TOLLBOOTH_PROTOCOL: &str = "x-tollbooth-protocol";
pub const X_TOLLBOOTH_SIGNATURE: &str = "x-tollbooth-signature";
pub const X_PAYMENT_PROTOCOL: &str = "x-payment-protocol";
pub const X_PAYMENT_SIGNATURE: &str = "x-payment-signature";
pub const X_TOLLBOOTH_EVENT: &str = "x-tollbooth-event";
pub const X_PAYMENT_CREDENTIAL: &str = "x-payment-credential";
pub const X_TOLLBOOTH_SESSION_ID: &str = "x-tollbooth-session-id";

/// Inject Tollbooth headers into a proxied request based on a payment receipt.
/// Optionally includes an event type header for inline integrations.
pub fn inject_receipt_headers(headers: &mut HeaderMap, receipt: &PaymentReceipt) {
    inject_receipt_headers_with_event(headers, receipt, None);
}

/// Inject Tollbooth headers with an optional event type.
pub fn inject_receipt_headers_with_event(
    headers: &mut HeaderMap,
    receipt: &PaymentReceipt,
    event_type: Option<&str>,
) {
    if let Ok(v) = HeaderValue::from_str("true") {
        headers.insert(HeaderName::from_static(X_TOLLBOOTH_VERIFIED), v);
    }
    if let Ok(v) = HeaderValue::from_str(&receipt.amount) {
        headers.insert(HeaderName::from_static(X_TOLLBOOTH_AMOUNT), v);
    }
    if let Ok(v) = HeaderValue::from_str(&receipt.ui_amount) {
        headers.insert(HeaderName::from_static(X_TOLLBOOTH_UI_AMOUNT), v);
    }
    if let Ok(v) = HeaderValue::from_str(&receipt.payer) {
        headers.insert(HeaderName::from_static(X_TOLLBOOTH_PAYER), v);
    }
    if let Ok(v) = HeaderValue::from_str(&receipt.protocol.to_string()) {
        headers.insert(HeaderName::from_static(X_TOLLBOOTH_PROTOCOL), v);
    }
    if let Ok(v) = HeaderValue::from_str(&receipt.signature) {
        headers.insert(HeaderName::from_static(X_TOLLBOOTH_SIGNATURE), v);
    }
    if let Some(event) = event_type
        && let Ok(v) = HeaderValue::from_str(event)
    {
        headers.insert(HeaderName::from_static(X_TOLLBOOTH_EVENT), v);
    }
    if let Some(ref sid) = receipt.session_id
        && let Ok(v) = HeaderValue::from_str(sid)
    {
        headers.insert(HeaderName::from_static(X_TOLLBOOTH_SESSION_ID), v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spl_tollbooth_core::types::ProtocolKind;

    fn make_receipt(session_id: Option<&str>) -> PaymentReceipt {
        PaymentReceipt {
            protocol: ProtocolKind::Mpp,
            signature: "5VERv8NMvzbJMEkV8xnrLkEaWRtSz9CosKDYjCJjBRnbJLgp8uirBgmQpjKhoR4tjF3ZpRzrFmBV6UjKdiSZkQUW".into(),
            amount: "1000".into(),
            ui_amount: "0.001".into(),
            mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".into(),
            payer: "11111111111111111111111111111111".into(),
            recipient: "22222222222222222222222222222222".into(),
            timestamp: 1700000000,
            session_id: session_id.map(String::from),
        }
    }

    #[test]
    fn inject_receipt_headers_sets_all_expected_headers() {
        let receipt = make_receipt(None);
        let mut headers = HeaderMap::new();
        inject_receipt_headers(&mut headers, &receipt);

        assert_eq!(headers.get(X_TOLLBOOTH_VERIFIED).unwrap(), "true");
        assert_eq!(headers.get(X_TOLLBOOTH_AMOUNT).unwrap(), "1000");
        assert_eq!(headers.get(X_TOLLBOOTH_UI_AMOUNT).unwrap(), "0.001");
        assert_eq!(
            headers.get(X_TOLLBOOTH_PAYER).unwrap(),
            "11111111111111111111111111111111"
        );
        assert_eq!(headers.get(X_TOLLBOOTH_PROTOCOL).unwrap(), "mpp");
        assert_eq!(
            headers.get(X_TOLLBOOTH_SIGNATURE).unwrap(),
            "5VERv8NMvzbJMEkV8xnrLkEaWRtSz9CosKDYjCJjBRnbJLgp8uirBgmQpjKhoR4tjF3ZpRzrFmBV6UjKdiSZkQUW"
        );
    }

    #[test]
    fn session_id_header_set_when_present() {
        let receipt = make_receipt(Some("sess-123"));
        let mut headers = HeaderMap::new();
        inject_receipt_headers(&mut headers, &receipt);

        assert_eq!(headers.get(X_TOLLBOOTH_SESSION_ID).unwrap(), "sess-123");
    }

    #[test]
    fn session_id_header_absent_when_none() {
        let receipt = make_receipt(None);
        let mut headers = HeaderMap::new();
        inject_receipt_headers(&mut headers, &receipt);

        assert!(headers.get(X_TOLLBOOTH_SESSION_ID).is_none());
    }

    #[test]
    fn event_type_header_set_when_provided() {
        let receipt = make_receipt(None);
        let mut headers = HeaderMap::new();
        inject_receipt_headers_with_event(&mut headers, &receipt, Some("payment.completed"));

        assert_eq!(headers.get(X_TOLLBOOTH_EVENT).unwrap(), "payment.completed");
    }

    #[test]
    fn event_type_header_absent_when_none() {
        let receipt = make_receipt(None);
        let mut headers = HeaderMap::new();
        inject_receipt_headers_with_event(&mut headers, &receipt, None);

        assert!(headers.get(X_TOLLBOOTH_EVENT).is_none());
    }

    #[test]
    fn mpp_protocol_renders_correctly() {
        let mut receipt = make_receipt(None);
        receipt.protocol = ProtocolKind::Mpp;
        let mut headers = HeaderMap::new();
        inject_receipt_headers(&mut headers, &receipt);

        assert_eq!(headers.get(X_TOLLBOOTH_PROTOCOL).unwrap(), "mpp");
    }
}
