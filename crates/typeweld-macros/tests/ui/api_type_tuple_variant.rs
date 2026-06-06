#[derive(typeweld_macros::ApiType)]
#[serde(tag = "_tag")]
enum UserEvent {
    Renamed(String),
}

fn main() {}
