#[derive(api_macros::ApiType)]
struct Profile {
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: String,
}

fn main() {}
