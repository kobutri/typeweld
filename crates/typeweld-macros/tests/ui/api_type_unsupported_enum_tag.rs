#[derive(typeweld_macros::ApiType)]
#[serde(tag = "kind")]
enum UserEvent {
    Created,
}

fn main() {}
