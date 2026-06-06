#[derive(api_macros::ApiType)]
#[serde(untagged)]
enum UserEvent {
    Renamed { name: String },
}

fn main() {}
