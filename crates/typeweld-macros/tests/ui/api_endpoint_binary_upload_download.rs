#[typeweld_macros::api(method = "POST", path = "/duplex")]
async fn upload_and_download(
    body: typeweld_core::Binary<bytes::Bytes>,
) -> typeweld_core::Binary<bytes::Bytes> {
    let _ = body;
    unimplemented!()
}

fn main() {}
