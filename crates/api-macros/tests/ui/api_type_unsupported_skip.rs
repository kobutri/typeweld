#[derive(api_macros::ApiType)]
struct Profile {
    #[serde(skip)]
    display_name: String,
}

fn main() {}
