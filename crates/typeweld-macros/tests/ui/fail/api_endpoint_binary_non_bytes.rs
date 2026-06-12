use typeweld::{api, Binary, NoContent};

#[api(post, "/blobs")]
pub async fn upload(data: Binary<Vec<u8>>) -> NoContent {
    unimplemented!()
}

fn main() {}
