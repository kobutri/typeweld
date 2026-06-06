#[api_macros::api(method = "GET", path = "/upload")]
async fn upload_file(body: api_core::Binary<bytes::Bytes>) -> api_core::NoContent {
    let _ = body;
    api_core::NoContent
}

fn main() {}
