use api_axum::{method_router, router, Sse};
use api_core::{api_module, ApiType};
use futures_util::stream;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, api_macros::ApiType)]
#[serde(rename_all = "camelCase")]
struct UserEvent {
    id: i64,
    kind: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, api_macros::ApiError)]
#[serde(rename_all = "camelCase")]
enum EventError {
    #[api_error(status = 401)]
    Unauthorized,
}

type EventStream = stream::Iter<std::array::IntoIter<Result<UserEvent, EventError>, 2>>;

#[api_macros::api(method = "SSE", path = "/events")]
fn events() -> Result<Sse<UserEvent, EventStream>, EventError> {
    Ok(Sse::new(stream::iter([
        Ok(UserEvent {
            id: 1,
            kind: "connected".to_owned(),
        }),
        Ok(UserEvent {
            id: 2,
            kind: "ready".to_owned(),
        }),
    ])))
}

fn main() {
    let module = api_module!(name = "events", endpoints = [__api_endpoint_events]);
    let _app = router(module)
        .route(
            __api_endpoint_events(),
            method_router(api_core::ir::HttpMethod::Get, || async {
                api_axum::success_or_error(events())
            }),
        )
        .into_router();

    println!("registered SSE endpoint /events");
}
