struct External<T>(T);

#[derive(typeweld_macros::ApiType)]
struct User {
    external: External<String>,
}

fn main() {}
