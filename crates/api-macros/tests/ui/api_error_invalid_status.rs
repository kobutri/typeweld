#[derive(api_macros::ApiError)]
enum CreateUserError {
    #[api_error(status = 200)]
    NotFound,
}

fn main() {}
