struct NotApiType;

#[derive(api_macros::ApiType)]
struct User {
    unsupported: NotApiType,
}

fn main() {}
