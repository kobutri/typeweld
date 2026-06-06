#[typeweld_macros::api(method = "GET", path = "/upload")]
async fn upload_file(body: typeweld_core::Binary<bytes::Bytes>) -> typeweld_core::NoContent {
    let _ = body;
    typeweld_core::NoContent
}

fn main() {}
