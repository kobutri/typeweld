#[api_macros::api(method = "POST", path = "/duplex")]
async fn upload_and_download(
    body: api_core::Binary<bytes::Bytes>,
) -> api_core::Binary<bytes::Bytes> {
    let _ = body;
    unimplemented!()
}

fn main() {}
