#[typeweld_macros::api(method = "GET", path = "/users")]
async fn get_user() -> String {
    String::new()
}

fn main() {}
