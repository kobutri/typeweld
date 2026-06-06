#[derive(api_macros::ApiType)]
struct Profile {
    #[serde(default)]
    display_name: String,
}

fn main() {}
