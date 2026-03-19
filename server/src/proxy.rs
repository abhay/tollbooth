use axum::Router;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use spl_tollbooth_core::config::{RouteEntry, TollboothConfig};

use crate::gate::{self, GateResult};
use crate::headers;
use crate::negotiate::negotiate_protocol;
use crate::relay::{prepare_handler, relay_handler};
use crate::state::AppState;

/// Reverse proxy server that payment-gates any backend.
pub struct ProxyServer {
    pub config: TollboothConfig,
    pub state: AppState,
}

impl ProxyServer {
    pub async fn from_config(config: TollboothConfig, state: AppState) -> Self {
        Self { config, state }
    }

    pub fn router(&self) -> Router {
        let state = self.state.clone();

        let relay_routes = Router::new()
            .route("/relay", post(relay_handler))
            .route("/relay/prepare", post(prepare_handler))
            .layer(DefaultBodyLimit::max(4096));

        Router::new()
            .merge(relay_routes)
            .route("/metrics", get(metrics_handler))
            .fallback(proxy_handler)
            .with_state(state)
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let listen = &self.config.server.listen;
        let listener = tokio::net::TcpListener::bind(listen).await?;
        tracing::info!("tollbooth listening on {listen}");
        axum::serve(listener, self.router()).await?;
        Ok(())
    }
}

/// Match a request path against configured routes.
/// Supports exact match and simple wildcard suffix (e.g. "/api/data/*").
fn match_route<'a>(path: &str, method: &str, routes: &'a [RouteEntry]) -> Option<&'a RouteEntry> {
    for route in routes {
        // Check method filter
        if let Some(ref route_method) = route.method
            && !route_method.eq_ignore_ascii_case(method)
        {
            continue;
        }

        // Exact match
        if route.path == path {
            return Some(route);
        }

        // Wildcard suffix: "/api/data/*" matches "/api/data/anything"
        if route.path.ends_with('*') {
            let prefix = &route.path[..route.path.len() - 1];
            if path.starts_with(prefix) {
                return Some(route);
            }
        }
    }
    None
}

/// Main proxy handler: matches routes, gates payments, forwards to upstream.
async fn proxy_handler(State(state): State<AppState>, request: Request) -> Response {
    let path = request.uri().path().to_string();
    let method_str = request.method().as_str().to_uppercase();
    let query = request
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();

    // Match against configured routes
    let matched_route = match_route(&path, &method_str, &state.routes);

    if let Some(route) = matched_route {
        let protocol = negotiate_protocol(request.headers(), &state.protocols);
        let price = spl_tollbooth_core::types::TokenAmount::from_display(
            &route.price,
            state.mint,
            state.decimals,
        );
        let price_raw = price.map(|p| p.raw).unwrap_or(0);

        match gate::check_payment(&state, request.headers(), route, &path, protocol, price_raw)
            .await
        {
            GateResult::Verified(receipt, event_type) => {
                let mut resp =
                    forward_to_upstream(&state, &method_str, &path, &query, request).await;
                headers::inject_receipt_headers_with_event(
                    resp.headers_mut(),
                    &receipt,
                    Some(&event_type),
                );
                return resp;
            }
            GateResult::Challenge(resp) | GateResult::Failed(resp) => return resp,
        }
    }

    // No route matched, pass through to upstream without payment gating
    forward_to_upstream(&state, &method_str, &path, &query, request).await
}

/// Forward request to upstream, preserving method, headers, and body.
async fn forward_to_upstream(
    state: &AppState,
    method: &str,
    path: &str,
    query: &str,
    request: Request,
) -> Response {
    let upstream_url = format!("{}{path}{query}", state.upstream);
    let client = &state.http_client;

    let reqwest_method = match method {
        "POST" => reqwest::Method::POST,
        "PUT" => reqwest::Method::PUT,
        "DELETE" => reqwest::Method::DELETE,
        "PATCH" => reqwest::Method::PATCH,
        "HEAD" => reqwest::Method::HEAD,
        "OPTIONS" => reqwest::Method::OPTIONS,
        _ => reqwest::Method::GET,
    };

    let mut upstream_req = client.request(reqwest_method.clone(), &upstream_url);

    // Forward headers (excluding hop-by-hop and payment-credential headers)
    for (name, value) in request.headers() {
        let name_str = name.as_str();
        if matches!(
            name_str,
            "host"
                | "connection"
                | "transfer-encoding"
                | "keep-alive"
                | "x-payment-signature"
                | "x-payment-credential"
                | "x-payment-protocol"
        ) {
            continue;
        }
        if let Ok(v) = value.to_str() {
            upstream_req = upstream_req.header(name_str, v);
        }
    }

    // Forward body for methods that have one
    if matches!(
        reqwest_method,
        reqwest::Method::POST | reqwest::Method::PUT | reqwest::Method::PATCH
    ) {
        let body = axum::body::to_bytes(request.into_body(), 10 * 1024 * 1024)
            .await
            .unwrap_or_default();
        upstream_req = upstream_req.body(body.to_vec());
    }

    match upstream_req.send().await {
        Ok(resp) => {
            let status =
                StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let resp_headers = resp.headers().clone();
            let body = resp.bytes().await.unwrap_or_default();

            let mut response = (status, body).into_response();
            // Forward upstream response headers
            for (name, value) in resp_headers.iter() {
                if let (Ok(n), Ok(v)) = (
                    HeaderName::try_from(name.as_str()),
                    HeaderValue::from_bytes(value.as_bytes()),
                ) {
                    response.headers_mut().insert(n, v);
                }
            }
            response
        }
        Err(e) => {
            tracing::error!("upstream request failed: {e}");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

/// GET /metrics handler.
async fn metrics_handler(State(state): State<AppState>) -> impl IntoResponse {
    let uptime = state.start_time.elapsed().as_secs();
    let now = spl_tollbooth_core::types::now_secs();

    // Query last 24h of key metrics from the store
    let metrics_to_query = [
        "payments.mpp.charge",
        "relay.signed",
        "relay.rejected",
        "relay.prepare",
        "relay.sol_spent",
        "revenue.total",
    ];

    let mut totals = serde_json::Map::new();
    for metric in &metrics_to_query {
        // Query all time (use the 1d table for long range)
        if let Ok(points) = state.store.query_metrics(metric, 0, now).await {
            let total: f64 = points.iter().map(|p| p.value).sum();
            if total > 0.0 {
                totals.insert(
                    metric.to_string(),
                    serde_json::Value::Number(
                        serde_json::Number::from_f64(total).unwrap_or(serde_json::Number::from(0)),
                    ),
                );
            }
        }
    }

    let mut last_24h = Vec::new();
    // Query 1-minute granularity for last 24h
    let from_24h = now - 86400;
    if let Ok(points) = state
        .store
        .query_metrics("payments.mpp.charge", from_24h, now)
        .await
    {
        for point in points {
            last_24h.push(serde_json::json!({
                "ts": point.ts,
                "payments.mpp.charge": point.value,
            }));
        }
    }

    axum::Json(serde_json::json!({
        "uptime_seconds": uptime,
        "totals": totals,
        "last_24h": last_24h
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use spl_tollbooth_core::config::RouteMode;

    #[test]
    fn match_exact_route() {
        let routes = vec![RouteEntry {
            path: "/api/joke".into(),
            method: Some("GET".into()),
            price: "0.001".into(),
            mode: RouteMode::Charge,
            deposit: None,
        }];
        assert!(match_route("/api/joke", "GET", &routes).is_some());
        assert!(match_route("/api/joke", "POST", &routes).is_none());
        assert!(match_route("/api/other", "GET", &routes).is_none());
    }

    #[test]
    fn match_wildcard_route() {
        let routes = vec![RouteEntry {
            path: "/api/data/*".into(),
            method: None,
            price: "0.01".into(),
            mode: RouteMode::Session,
            deposit: Some("0.1".into()),
        }];
        assert!(match_route("/api/data/page1", "GET", &routes).is_some());
        assert!(match_route("/api/data/", "POST", &routes).is_some());
        assert!(match_route("/api/other", "GET", &routes).is_none());
    }

    #[test]
    fn no_method_filter_matches_any() {
        let routes = vec![RouteEntry {
            path: "/api/thing".into(),
            method: None,
            price: "0.001".into(),
            mode: RouteMode::Charge,
            deposit: None,
        }];
        assert!(match_route("/api/thing", "GET", &routes).is_some());
        assert!(match_route("/api/thing", "POST", &routes).is_some());
    }
}
