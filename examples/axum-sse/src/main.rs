use api_axum::{ApiRouter, Sse};
use api_core::ApiType;
use futures_util::stream;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, api_macros::ApiType)]
#[serde(rename_all = "camelCase")]
struct UserEvent {
    id: i64,
    kind: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, api_macros::ApiError)]
#[serde(tag = "_tag", rename_all = "camelCase")]
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

#[api_macros::api_router]
fn routes() -> ApiRouter {
    ApiRouter::new("events").endpoint(events)
}

fn main() {
    let _app = routes().into_router();

    println!("registered SSE endpoint /events");
}
