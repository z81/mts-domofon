#[derive(serde::Serialize)]
pub(super) struct HealthResponse {
    status: &'static str,
}

pub(super) async fn live() -> axum::Json<HealthResponse> {
    axum::Json(HealthResponse { status: "ok" })
}
