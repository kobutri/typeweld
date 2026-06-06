#[derive(api_macros::ApiType)]
#[serde(tag = "_tag", content = "value")]
enum UserEvent {
    Renamed { name: String },
}

fn main() {}
