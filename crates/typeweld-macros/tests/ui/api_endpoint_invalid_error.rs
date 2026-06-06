use typeweld_core::Json;

#[derive(typeweld_macros::ApiType, serde::Serialize)]
struct User {
    id: i64,
}

#[derive(serde::Serialize)]
struct NotApiError;

#[typeweld_macros::api(method = "GET", path = "/users")]
async fn get_user() -> Result<Json<User>, NotApiError> {
    unimplemented!()
}

fn main() {}
