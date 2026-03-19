use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use spl_tollbooth_core::config::{RouteEntry, RouteMode};

use crate::gate::{self, GateResult};
use crate::headers;
use crate::negotiate::negotiate_protocol;
use crate::state::AppState;

/// Axum middleware that payment-gates requests.
pub async fn tollbooth_middleware(
    state: AppState,
    route_path: String,
    price_raw: u64,
    is_session: bool,
    request: Request,
    next: Next,
) -> Response {
    let protocol = negotiate_protocol(request.headers(), &state.protocols);

    // Build a RouteEntry so the gate can derive everything it needs.
    let route = RouteEntry {
        path: route_path.clone(),
        method: None,
        price: price_raw.to_string(),
        mode: if is_session {
            RouteMode::Session
        } else {
            RouteMode::Charge
        },
        deposit: None,
    };

    match gate::check_payment(
        &state,
        request.headers(),
        &route,
        &route_path,
        protocol,
        price_raw,
    )
    .await
    {
        GateResult::Verified(receipt, event_type) => {
            let mut response = next.run(request).await;
            headers::inject_receipt_headers_with_event(
                response.headers_mut(),
                &receipt,
                Some(&event_type),
            );
            response
        }
        GateResult::Challenge(resp) | GateResult::Failed(resp) => resp,
    }
}
