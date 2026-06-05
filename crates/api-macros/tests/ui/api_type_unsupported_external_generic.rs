struct External<T>(T);

#[derive(api_macros::ApiType)]
struct User {
    external: External<String>,
}

fn main() {}
