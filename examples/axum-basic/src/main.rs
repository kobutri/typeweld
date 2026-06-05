use api_core::Endpoint;

fn main() {
    let endpoint = Endpoint::new("GET", "/users/{id}");
    println!("{} {}", endpoint.method, endpoint.path);
}
