//! Monorepo example: endpoints in one crate, API types in another.

use api_types::models::{Widget, WidgetId};
use serde::{Deserialize, Serialize};
use typeweld::{api, api_router, Api, ApiError, ApiRouter, Json, Path};

#[derive(Clone, Debug, Deserialize, Serialize, ApiError)]
#[serde(tag = "_tag")]
pub enum WidgetError {
    #[status(404)]
    WidgetNotFound { id: WidgetId },
}

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
#[serde(rename_all = "camelCase")]
pub struct Inventory {
    pub widgets: Vec<Widget>,
    pub total: u32,
}

/// Fetch one widget.
#[api(get, "/widgets/{id}")]
pub async fn get_widget(id: Path<WidgetId>) -> Result<Json<Widget>, WidgetError> {
    Err(WidgetError::WidgetNotFound { id: *id })
}

/// The whole inventory.
#[api(get, "/inventory", allow_unused)]
pub async fn get_inventory() -> Json<Inventory> {
    Json(Inventory {
        widgets: Vec::new(),
        total: 0,
    })
}

#[api_router]
pub fn widget_routes() -> ApiRouter {
    ApiRouter::new().endpoint(get_widget)
}

#[api_router]
pub fn routes() -> ApiRouter {
    ApiRouter::new()
        .nest("/api/v1", widget_routes())
        .endpoint(get_inventory)
}

#[tokio::main]
async fn main() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:39124")
        .await
        .expect("bind");
    axum::serve(listener, routes().into_router())
        .await
        .expect("serve");
}
