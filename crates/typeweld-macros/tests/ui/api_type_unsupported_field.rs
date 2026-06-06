struct NotApiType;

#[derive(typeweld_macros::ApiType)]
struct User {
    unsupported: NotApiType,
}

fn main() {}
