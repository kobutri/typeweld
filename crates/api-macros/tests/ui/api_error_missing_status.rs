#[derive(api_macros::ApiError)]
#[serde(tag = "_tag")]
enum CreateUserError {
    NotFound,
}

fn main() {}
