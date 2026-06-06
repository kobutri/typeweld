use typeweld_core::{ir::HttpMethod, Endpoint};

fn main() {
    let endpoint = Endpoint::new(HttpMethod::Get, "/users/{id}");
    println!("{} {}", endpoint.method.as_str(), endpoint.route.0);
}
