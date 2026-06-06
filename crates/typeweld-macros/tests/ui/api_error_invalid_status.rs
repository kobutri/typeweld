#[derive(typeweld_macros::ApiError)]
#[serde(tag = "_tag")]
enum CreateUserError {
    #[api_error(status = 200)]
    NotFound,
}

fn main() {}
